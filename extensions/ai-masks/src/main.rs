//! The AI masks extension's program, which RAWmakase starts at the first
//! selection and stops when it has been idle a while.
fn main() -> std::io::Result<()> {
    rawmakase_ai_masks::extension(rawmakase_ai_masks::Options::default()).run()
}
