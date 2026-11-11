//! The reader's handle framing over the real transport (a socket pair on Linux
//! and macOS, pipes on Windows), with the open handle count back at its
//! baseline after each case. These tests share one lock: they count the
//! process's handles, so none of them runs beside another.
use super::*;
use crate::run::OnFault;
use crate::{Context, Extension, MaskAsk, MaskGenerator, ServeOptions};
use rawmakase_extension_protocol::buffer::{
    Alpha, Meaning, Primaries, Region, Sample, Space, Transfer, White,
};
use rawmakase_extension_protocol::framing::LineReader;
use rawmakase_extension_protocol::message::Handles;
use rawmakase_extension_protocol::method::{CommandRun, Hello, MaskGenerate, MaskGenerated};
use rawmakase_extension_protocol::{ErrorCode, MAX_LINE, Message, PROTOCOL, RemoteError};
use serde_json::{Value, json};
use std::io::Write;
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

static COUNTING: Mutex<()> = Mutex::new(());

/// A named way to break the framing.
type Case<'a> = (&'a str, &'a dyn Fn(&Wire));

const WIDTH: u32 = 8;
const HEIGHT: u32 = 4;

/// Fills the coverage with the image's first byte.
struct Fill;
impl MaskGenerator for Fill {
    fn generate(&mut self, _cx: &Context, ask: MaskAsk<'_>) -> Result<MaskGenerated, RemoteError> {
        ask.coverage.fill(ask.image[0]);
        Ok(MaskGenerated {
            model: "fill".into(),
            model_key: "fill/1".into(),
            empty: None,
        })
    }
}

fn extension() -> Extension {
    Extension::new().mask_generator("fill", Fill).command(
        "wait",
        |cx: &Context, _run: CommandRun| {
            let start = Instant::now();
            while !cx.is_cancelled() && start.elapsed() < Duration::from_secs(20) {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(RemoteError::new(ErrorCode::Cancelled, "stopped"))
        },
    )
}

fn mask_request() -> MaskGenerate {
    let region = Region {
        space: Space::ImageFrame,
        rect: [0.0, 0.0, 1.0, 1.0],
        scale: 1.0,
        overlap: 0,
    };
    let image = Buffer {
        handle: 0,
        bytes: u64::from(WIDTH * HEIGHT * 3),
        width: WIDTH,
        height: HEIGHT,
        stride: WIDTH * 3,
        channels: 3,
        sample: Sample::U8,
        meaning: Meaning::Color {
            primaries: Primaries::Srgb,
            white: White::D65,
            transfer: Transfer::Srgb,
            alpha: Alpha::None,
        },
        region,
        access: Access::Read,
    };
    let coverage = Buffer {
        handle: 1,
        bytes: u64::from(WIDTH * HEIGHT),
        stride: WIDTH,
        channels: 1,
        meaning: Meaning::Coverage,
        access: Access::Write,
        ..image
    };
    MaskGenerate {
        generator: "fill".into(),
        input: None,
        image,
        coverage,
        image_key: "k".into(),
    }
}

fn mask_message(id: u64) -> Message {
    Message::request(
        id,
        "mask.generate",
        serde_json::to_value(mask_request()).unwrap(),
    )
}

fn line(message: &Message, handles: &Handles) -> Vec<u8> {
    let mut line = message.encode_with(handles).unwrap();
    line.push(b'\n');
    line
}

fn hello_line() -> Vec<u8> {
    let hello = Hello {
        protocol: PROTOCOL,
        app_version: "0.0.0".into(),
        extension: "test.extension".into(),
        settings: Default::default(),
        contributions: Vec::new(),
        data_dir: std::env::temp_dir(),
        cache_dir: std::env::temp_dir(),
        session: 1,
    };
    line(
        &Message::request(1, "hello", serde_json::to_value(hello).unwrap()),
        &Handles::default(),
    )
}

/// The buffers of one mask request: an image whose first byte is `value`, and
/// a coverage.
fn buffers(value: u8) -> [Memory; 2] {
    let mut image = vec![0u8; (WIDTH * HEIGHT * 3) as usize];
    image[0] = value;
    [
        Memory::input(&image).unwrap(),
        Memory::output((WIDTH * HEIGHT) as usize).unwrap(),
    ]
}

/// The app's end of a channel and the extension served on the other.
struct Wire {
    #[cfg(unix)]
    app: std::os::unix::net::UnixStream,
    #[cfg(windows)]
    app: Option<std::io::PipeWriter>,
    replies: LineReader<Box<dyn std::io::Read + Send>>,
    program: Option<(Box<dyn Receive>, Box<dyn Write + Send>)>,
    serving: Option<JoinHandle<io::Result<()>>>,
}

impl Wire {
    /// A channel whose program isn't served yet, so what is sent before
    /// [`Wire::serve`] arrives in one read.
    fn new() -> Self {
        #[cfg(unix)]
        {
            let (app, program) = std::os::unix::net::UnixStream::pair().unwrap();
            Self {
                replies: LineReader::new(Box::new(app.try_clone().unwrap())),
                app,
                program: Some((Box::new(program.try_clone().unwrap()), Box::new(program))),
                serving: None,
            }
        }
        #[cfg(windows)]
        {
            let (app_reader, program_writer) = std::io::pipe().unwrap();
            let (program_reader, app_writer) = std::io::pipe().unwrap();
            Self {
                replies: LineReader::new(Box::new(app_reader)),
                app: Some(app_writer),
                program: Some((Box::new(Plain(program_reader)), Box::new(program_writer))),
                serving: None,
            }
        }
    }

    fn serve(&mut self, api: u32) {
        let (reader, writer) = self.program.take().unwrap();
        self.serving = Some(std::thread::spawn(move || {
            extension().serve_on(reader, writer, ServeOptions { api }, OnFault::Close)
        }));
    }

    /// Sends `bytes` with the buffers' handles, as the app does: beside the
    /// first byte on Linux and macOS. Windows lists them in the line, so
    /// there they are left to [`Wire::send_message`].
    fn send(&self, bytes: &[u8], buffers: &[&Memory]) {
        self.try_send(bytes, buffers).unwrap();
    }

    /// [`Wire::send`] to a program that may have ended serving already.
    fn try_send(&self, bytes: &[u8], buffers: &[&Memory]) -> io::Result<()> {
        #[cfg(unix)]
        {
            let fds: Vec<_> = buffers.iter().map(|b| b.handle()).collect();
            send(&self.app, bytes, &fds)
        }
        #[cfg(windows)]
        {
            assert!(buffers.is_empty(), "Windows lists handles in the line");
            let mut app = self.app.as_ref().unwrap();
            app.write_all(bytes)
        }
    }

    /// Sends a message with these buffers as its handles, declared. On
    /// Windows, returns the duplicates' values, which [`closed`] checks.
    fn send_message(&self, message: &Message, buffers: &[&Memory]) -> Vec<u64> {
        #[cfg(unix)]
        {
            let handles = Handles {
                handles: buffers.len() as u32,
                hv: None,
            };
            self.send(&line(message, &handles), buffers);
            Vec::new()
        }
        #[cfg(windows)]
        {
            let values: Vec<u64> = buffers.iter().map(|b| b.duplicate().unwrap()).collect();
            let handles = Handles {
                handles: values.len() as u32,
                hv: (!values.is_empty()).then(|| values.clone()),
            };
            self.send(&line(message, &handles), &[]);
            values
        }
    }

    fn next(&mut self) -> Option<Message> {
        match self.replies.next_message() {
            Ok(Some(Ok(message))) => Some(message),
            _ => None,
        }
    }

    /// The reply to request `id`, skipping anything else.
    fn reply(&mut self, id: u64) -> Option<Result<Value, RemoteError>> {
        loop {
            match self.next()? {
                Message::Ok { id: i, v } if i == id => return Some(Ok(v)),
                Message::Err {
                    id: i,
                    code,
                    message,
                } if i == id => {
                    return Some(Err(RemoteError { code, message }));
                }
                _ => {}
            }
        }
    }

    fn close(&mut self) {
        #[cfg(unix)]
        let _ = self.app.shutdown(std::net::Shutdown::Write);
        #[cfg(windows)]
        drop(self.app.take());
    }

    /// Closes the app's side and returns what serving returned.
    fn finish(mut self) -> io::Result<()> {
        self.close();
        let result = self.serving.take().unwrap().join().unwrap();
        // Everything the program read, until the end.
        while self.next().is_some() {}
        result
    }
}

/// Whether every handle a message passed is closed again. Windows only: there
/// the process's handle count also moves with the test harness's threads, so
/// the passed handles are checked themselves.
#[cfg(windows)]
fn closed(values: &[u64]) -> bool {
    use windows_sys::Win32::Foundation::GetHandleInformation;
    values.iter().all(|&value| {
        let mut flags = 0;
        // SAFETY: asks about a handle value of this process; a closed one
        // fails.
        unsafe { GetHandleInformation(value as usize as _, &mut flags) == 0 }
    })
}

/// The descriptor count once it holds still: an earlier case's reader thread
/// lets go of its end a moment after that case returns.
#[cfg(unix)]
fn baseline() -> usize {
    let mut count = open_handles();
    loop {
        std::thread::sleep(Duration::from_millis(20));
        let now = open_handles();
        if now == count {
            return count;
        }
        count = now;
    }
}

/// Waits for the handle count to come back to `baseline`: the program's reader
/// thread lets go of its end once it has read the end of the stream.
#[cfg(unix)]
fn settles(baseline: usize) {
    let start = Instant::now();
    while open_handles() != baseline {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{} handles open, {baseline} before",
            open_handles()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn fault(result: io::Result<()>) -> String {
    let error = result.expect_err("a framing fault ends serving");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{error}");
    error.to_string()
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    COUNTING.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn a_mask_request_maps_its_buffers_and_unmaps_them_before_replying() {
    let _lock = lock();
    #[cfg(unix)]
    let baseline = baseline();
    {
        let mut wire = Wire::new();
        wire.serve(2);
        wire.send(&hello_line(), &[]);
        wire.reply(1).unwrap().unwrap();
        let [image, coverage] = buffers(200);
        let passed = wire.send_message(&mask_message(2), &[&image, &coverage]);
        wire.reply(2).unwrap().unwrap();
        assert_eq!(mapped_buffers(), 0);
        assert!(coverage.contents().iter().all(|&b| b == 200));
        #[cfg(windows)]
        assert!(closed(&passed));
        let _ = passed;
        wire.finish().unwrap();
    }
    #[cfg(unix)]
    settles(baseline);
}

#[test]
fn a_framing_fault_in_api_2_ends_serving() {
    let _lock = lock();
    #[cfg(unix)]
    let baseline = baseline();
    let cases: [Case<'_>; 3] = [
        ("malformed", &|wire| {
            wire.send(b"{\"t\":\"req\",\"id\":5,\n", &[])
        }),
        ("oversized", &|wire| {
            let mut big = vec![b'x'; MAX_LINE + 10];
            big.push(b'\n');
            wire.send(&big, &[]);
        }),
        ("cut off", &|wire| wire.send(b"{\"t\":\"event\"", &[])),
    ];
    for (name, send) in cases {
        let mut wire = Wire::new();
        wire.serve(2);
        wire.send(&hello_line(), &[]);
        wire.reply(1).unwrap().unwrap();
        send(&wire);
        if name == "cut off" {
            wire.close();
        }
        // Nothing is answered: the program is gone.
        assert_eq!(wire.next(), None, "{name}");
        fault(wire.finish());
    }
    #[cfg(unix)]
    settles(baseline);
}

#[test]
fn api_1_recovers_from_malformed_and_oversized_lines_and_refuses_handles() {
    let _lock = lock();
    #[cfg(unix)]
    let baseline = baseline();
    {
        let mut wire = Wire::new();
        wire.serve(1);
        wire.send(&hello_line(), &[]);
        wire.reply(1).unwrap().unwrap();
        // A malformed request with a readable id is answered, then reading
        // goes on.
        wire.send(b"{\"t\":\"req\",\"id\":5}\n", &[]);
        assert_eq!(
            wire.reply(5).unwrap().unwrap_err().code,
            ErrorCode::InvalidRequest
        );
        // An oversized line is skipped.
        let mut big = vec![b'x'; MAX_LINE + 10];
        big.push(b'\n');
        wire.send(&big, &[]);
        // A request declaring handles is refused, its handles closed.
        let [image, coverage] = buffers(1);
        let passed = wire.send_message(&mask_message(6), &[&image, &coverage]);
        let refused = wire.reply(6).unwrap().unwrap_err();
        assert_eq!(refused.code, ErrorCode::InvalidRequest);
        assert!(refused.message.contains("API 1"), "{refused}");
        let run = json!({"command": "missing"});
        wire.send(
            &line(
                &Message::request(7, "command.run", run),
                &Handles::default(),
            ),
            &[],
        );
        assert_eq!(
            wire.reply(7).unwrap().unwrap_err().code,
            ErrorCode::InvalidRequest
        );
        wire.finish().unwrap();
        // Windows: the duplicates the refused request listed were never
        // adopted. The app never sends them to an API 1 program (it fails
        // such a request itself), so the test closes them.
        #[cfg(windows)]
        close(&passed);
        let _ = passed;
    }
    #[cfg(unix)]
    settles(baseline);
}

#[test]
fn a_queued_request_cancelled_is_answered_at_once_and_its_handles_close() {
    let _lock = lock();
    #[cfg(unix)]
    let baseline = baseline();
    {
        let mut wire = Wire::new();
        wire.serve(2);
        wire.send(&hello_line(), &[]);
        wire.reply(1).unwrap().unwrap();
        let run = json!({"command": "wait"});
        wire.send(
            &line(
                &Message::request(2, "command.run", run),
                &Handles::default(),
            ),
            &[],
        );
        let [image, coverage] = buffers(9);
        #[cfg(unix)]
        let before = open_handles();
        let passed = wire.send_message(&mask_message(3), &[&image, &coverage]);
        wire.send(&line(&Message::Cancel { id: 3 }, &Handles::default()), &[]);
        // Answered while the command still runs, its handles already closed.
        assert_eq!(
            wire.reply(3).unwrap().unwrap_err().code,
            ErrorCode::Cancelled
        );
        #[cfg(unix)]
        assert_eq!(open_handles(), before);
        #[cfg(windows)]
        assert!(closed(&passed));
        let _ = passed;
        wire.send(&line(&Message::Cancel { id: 2 }, &Handles::default()), &[]);
        assert_eq!(
            wire.reply(2).unwrap().unwrap_err().code,
            ErrorCode::Cancelled
        );
        assert!(coverage.contents().iter().all(|&b| b == 0), "never ran");
        wire.finish().unwrap();
    }
    #[cfg(unix)]
    settles(baseline);
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::fd::{AsFd, AsRawFd};

    /// A named way to break the framing with a request's two buffers.
    type WithHandles<'a> = (&'a str, &'a dyn Fn(&Wire, &[Memory; 2]));

    /// A mask request whose line goes in two writes: its first byte with
    /// the handles, then a pause, then the rest.
    #[test]
    fn a_handle_line_split_across_reads_is_read_whole() {
        let _lock = lock();
        let baseline = baseline();
        {
            let mut wire = Wire::new();
            wire.serve(2);
            wire.send(&hello_line(), &[]);
            wire.reply(1).unwrap().unwrap();
            let [image, coverage] = buffers(77);
            let bytes = line(
                &mask_message(2),
                &Handles {
                    handles: 2,
                    hv: None,
                },
            );
            wire.send(&bytes[..1], &[&image, &coverage]);
            std::thread::sleep(Duration::from_millis(50));
            wire.send(&bytes[1..], &[]);
            wire.reply(2).unwrap().unwrap();
            assert!(coverage.contents().iter().all(|&b| b == 77));
            wire.finish().unwrap();
        }
        settles(baseline);
    }

    /// Sent before the program reads, so its first read takes the plain line
    /// and the handle line together; and then a whole line with the first
    /// byte of a handle line, whose rest comes after a pause.
    #[test]
    fn lines_read_together_or_partly_take_their_own_handles() {
        let _lock = lock();
        let baseline = baseline();
        {
            let mut wire = Wire::new();
            let [image, coverage] = buffers(5);
            let mut first = hello_line();
            first.extend(line(
                &Message::event("noop", Value::Null),
                &Handles::default(),
            ));
            wire.send(&first, &[]);
            let handles = Handles {
                handles: 2,
                hv: None,
            };
            wire.send(&line(&mask_message(2), &handles), &[&image, &coverage]);
            let [image2, coverage2] = buffers(6);
            let second = line(&mask_message(3), &handles);
            let mut whole = line(&Message::event("noop", Value::Null), &Handles::default());
            whole.push(second[0]);
            wire.send(&whole, &[&image2, &coverage2]);
            wire.serve(2);
            wire.reply(1).unwrap().unwrap();
            wire.reply(2).unwrap().unwrap();
            // The partial line keeps its handles queued: no fault while it waits.
            std::thread::sleep(Duration::from_millis(50));
            wire.send(&second[1..], &[]);
            wire.reply(3).unwrap().unwrap();
            assert!(coverage.contents().iter().all(|&b| b == 5));
            assert!(coverage2.contents().iter().all(|&b| b == 6));
            wire.finish().unwrap();
        }
        settles(baseline);
    }

    /// Each case breaks the handle framing once, after a good hello.
    #[test]
    fn handles_short_of_or_beyond_their_line_end_serving() {
        let _lock = lock();
        let baseline = baseline();
        let declared = |n| Handles {
            handles: n,
            hv: None,
        };
        let cases: [WithHandles<'_>; 4] = [
            ("missing, then valid", &|wire, [image, coverage]| {
                wire.send(&line(&mask_message(2), &declared(2)), &[image]);
                // The program may have ended serving at the first already.
                let valid = line(&mask_message(3), &declared(2));
                let _ = wire.try_send(&valid, &[image, coverage]);
            }),
            ("surplus at a boundary", &|wire, [image, _]| {
                let event = line(&Message::event("noop", Value::Null), &Handles::default());
                wire.send(&event, &[image]);
            }),
            ("malformed with handles", &|wire, [image, coverage]| {
                wire.send(
                    b"{\"t\":\"req\",\"id\":2,\"handles\":2,\n",
                    &[image, coverage],
                );
            }),
            ("cut off with handles", &|wire, [image, coverage]| {
                let bytes = line(&mask_message(2), &declared(2));
                wire.send(&bytes[..10], &[image, coverage]);
            }),
        ];
        for (name, send) in cases {
            let mut wire = Wire::new();
            wire.serve(2);
            wire.send(&hello_line(), &[]);
            wire.reply(1).unwrap().unwrap();
            let memory = buffers(1);
            send(&wire, &memory);
            if name.starts_with("cut off") {
                wire.close();
            }
            assert_eq!(wire.next(), None, "{name}");
            let why = fault(wire.finish());
            assert!(!why.is_empty(), "{name}");
            drop(memory);
            settles(baseline);
        }
    }

    /// Sixteen descriptors with a line declaring them, past `MAX_HANDLES`:
    /// the reader's room (512) takes them all, so nothing is cut off, and the
    /// count is the fault. Serving ends and every descriptor is closed.
    #[test]
    fn a_line_declaring_more_than_max_handles_ends_serving() {
        let _lock = lock();
        let baseline = baseline();
        {
            let mut wire = Wire::new();
            wire.serve(2);
            wire.send(&hello_line(), &[]);
            wire.reply(1).unwrap().unwrap();
            let memory: Vec<Memory> = (0..16).map(|_| Memory::output(16).unwrap()).collect();
            let fds: Vec<_> = memory.iter().map(Memory::handle).collect();
            let bytes = line(
                &mask_message(2),
                &Handles {
                    handles: 16,
                    hv: None,
                },
            );
            send(&wire.app, &bytes, &fds).unwrap();
            assert_eq!(wire.next(), None);
            assert!(fault(wire.finish()).contains("more than the 8"));
        }
        settles(baseline);
    }

    /// A request refused without running (here, before `hello`) closes its
    /// handles before it is answered, as a request cancelled between its
    /// claim and its start does: the reply may end the app's lease at once.
    #[test]
    fn a_request_refused_without_running_closes_its_handles_first() {
        let _lock = lock();
        let mut wire = Wire::new();
        wire.serve(2);
        let [image, coverage] = buffers(4);
        let before = open_handles();
        wire.send_message(&mask_message(2), &[&image, &coverage]);
        let error = wire.reply(2).unwrap().unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(open_handles(), before, "closed before the reply");
        wire.finish().unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_unsealed_input_is_refused() {
        let _lock = lock();
        let mut wire = Wire::new();
        wire.serve(2);
        wire.send(&hello_line(), &[]);
        wire.reply(1).unwrap().unwrap();
        let image = Memory::unsealed(&[3; (WIDTH * HEIGHT * 3) as usize]).unwrap();
        let coverage = Memory::output((WIDTH * HEIGHT) as usize).unwrap();
        wire.send_message(&mask_message(2), &[&image, &coverage]);
        let error = wire.reply(2).unwrap().unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("sealed"), "{error}");
        assert_eq!(mapped_buffers(), 0);
        wire.finish().unwrap();
    }

    #[test]
    fn received_descriptors_are_close_on_exec() {
        let _lock = lock();
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        let memory = Memory::output(16).unwrap();
        send(&a, b"x", &[memory.handle()]).unwrap();
        let mut fds = VecDeque::new();
        let (read, cut) = receive(b.as_fd(), &mut [0; 4], MAX_HANDLES_FOR_TEST, &mut fds).unwrap();
        assert_eq!((read, cut, fds.len()), (1, false, 1));
        // SAFETY: reads the flags of a descriptor this test owns.
        let flags = unsafe { libc::fcntl(fds[0].as_raw_fd(), libc::F_GETFD) };
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
    }

    const MAX_HANDLES_FOR_TEST: usize = 8;

    /// Sixteen descriptors against room for eight: the control data is cut
    /// off (`MSG_CTRUNC`), and only what the kernel returned is read, each
    /// a descriptor this process now owns. On macOS the kernel keeps the cut
    /// message's full `cmsg_len`; reading by it would run past the buffer.
    #[test]
    fn cut_off_control_data_is_read_no_further_than_returned() {
        let _lock = lock();
        let baseline = baseline();
        {
            let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
            let memory: Vec<Memory> = (0..16).map(|_| Memory::output(16).unwrap()).collect();
            let fds: Vec<_> = memory.iter().map(Memory::handle).collect();
            send(&a, b"x", &fds).unwrap();
            let mut received = VecDeque::new();
            let (read, cut) = receive(b.as_fd(), &mut [0; 4], 8, &mut received).unwrap();
            assert_eq!(read, 1);
            assert!(cut, "the control data was cut off");
            assert!(received.len() <= 8, "{} taken", received.len());
            #[cfg(target_os = "linux")]
            assert_eq!(received.len(), 8, "Linux installs what fits");
            for fd in &received {
                // SAFETY: reads the flags of a descriptor this test owns.
                assert!(unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) } >= 0);
            }
        }
        settles(baseline);
    }
}

#[cfg(windows)]
mod windows {
    use super::*;

    /// An `hv` that doesn't match `handles`, or lists a value twice.
    #[test]
    fn an_hv_list_off_its_count_ends_serving() {
        let _lock = lock();
        for (name, handles) in [
            (
                "short",
                Handles {
                    handles: 2,
                    hv: Some(vec![]),
                },
            ),
            (
                "missing",
                Handles {
                    handles: 1,
                    hv: None,
                },
            ),
            (
                "repeated",
                Handles {
                    handles: 2,
                    hv: Some(vec![4, 4]),
                },
            ),
        ] {
            let mut wire = Wire::new();
            wire.serve(2);
            wire.send(&hello_line(), &[]);
            wire.reply(1).unwrap().unwrap();
            wire.send(&line(&mask_message(2), &handles), &[]);
            assert_eq!(wire.next(), None, "{name}");
            fault(wire.finish());
        }
    }

    /// An input's duplicate maps for reading and never for writing.
    #[test]
    fn an_input_arrives_read_only() {
        let _lock = lock();
        let memory = Memory::input(&[1, 2, 3]).unwrap();
        let mut handle = adopt(&[memory.duplicate().unwrap()]);
        let handle = handle.pop().unwrap();
        assert!(View::new(&handle, 3, true).is_err());
        assert_eq!(View::new(&handle, 3, false).unwrap().bytes(), [1, 2, 3]);
    }
}
