#include <libraw/libraw.h>
#ifdef _WIN32
// After LibRaw, which includes winsock2.h ahead of windows.h as Windows requires.
#include <windows.h>
#include <string>
#endif
#ifdef _OPENMP
#include <omp.h>
#endif
#include <algorithm>
#include <climits>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <exception>
#include <memory>
#include <vector>

extern "C" {
struct Metadata {
    unsigned width, height, raw_width, raw_height, crop_width, crop_height, crop_left, crop_top;
    int flip, xtrans;
    unsigned fuji_dynamic_range;
    float iso, shutter, aperture, focal, wb[3], daylight_wb[3], matrix[9];
    char make[64], model[64];
    float cam_xyz[9];
    char lens[128];
    float focal_35mm;
    int highlight_tone_priority;
    float fuji_exposure_shift;
    // The camera's own stored Daylight, Shade, D65 and Fine Weather presets
    // (WB_Coeffs, as R, G, B), zero when the RAW has none. Per-unit calibration
    // divides a camera table reference by one of them; DNGs carry their
    // calibration instead. LibRaw files Olympus's 5300 K entry as Fine Weather.
    float preset_daylight[3], preset_shade[3], preset_d65[3], preset_fine_weather[3];
    // The visible image's offset in the raw data, and the camera's aspect-ratio
    // crop in raw coordinates, as crop_* are (cleft 0xffff when there is none).
    unsigned left_margin, top_margin;
    unsigned aspect_left, aspect_top, aspect_width, aspect_height;
    // The maker's numeric lens ID, the value the lens profiles' stCamera:LensID
    // holds too (Nikon's LensIDNumber, Pentax's LensType); 0 when the maker notes
    // have none.
    unsigned lens_id;
    // LibRaw's canonical name for bodies sold under several names (the Panasonic
    // DC-G95 is the DC-G99), which is often the one Adobe's profiles use.
    char normalized_make[64], normalized_model[64];
    // The DNG version, 0 for a camera's own format.
    unsigned dng_version;
    // A small raw (Canon sRAW and mRAW, Nikon sRAW), which LibRaw converts from YCbCr.
    int sraw;
};
// The unpacked sensor data and the levels development reads with it, for decode
// fingerprints. data and black point into the handle's buffers: data holds
// height rows of pitch values, of which the area development reads starts at
// left, top and is area_width by area_height; black LibRaw's cblack (4 channels, the pattern's
// rows and columns, then its values).
struct Sensor {
    const unsigned short* data;
    unsigned width, height, pitch, channels;
    unsigned left, top, area_width, area_height;
    const unsigned* black;
    unsigned base_black, black_size;
    unsigned white, linear_white[4];
    unsigned pattern_rows, pattern_cols;
    unsigned char pattern[16*16];
};
typedef int (*Cancel)(void*);
// RAWmakase's decoder of Nikon's High Efficiency raws (nikon_he.rs): the codestream
// and its length, then the raw buffer's width, height and pitch (in values). Returns
// nonzero, with err filled, when it fails.
typedef int (*HighEfficiency)(const unsigned char*, size_t, unsigned short*, unsigned, unsigned,
                              unsigned, char*);
}

// LibRaw retains its black subtraction and demosaicing. Only the common scale
// is reduced when necessary; its inverse is retained across the integer boundary.
class Raw : public LibRaw {
public:
    float decode_gain = 1;
    float scale_factor = 1;
    unsigned scale_clipped = 0;
    HighEfficiency high_efficiency = nullptr;
    char high_efficiency_error[512] = {};
    explicit Raw() : LibRaw() {}
    // Has unpack() decode the raw data with `decode` instead of LibRaw's decoder.
    void decode_high_efficiency(HighEfficiency decode) {
        high_efficiency = decode;
        load_raw = static_cast<void (LibRaw::*)()>(&Raw::high_efficiency_load_raw);
    }
    void high_efficiency_load_raw() {
        auto& unpacker = libraw_internal_data.unpacker_data;
        auto* input = libraw_internal_data.internal_data.input;
        const INT64 offset = unpacker.data_offset, size = unpacker.data_size;
        if (!high_efficiency || !input || offset < 0 || size <= 0 || size > INT_MAX
            || offset + size > input->size())
            throw LIBRAW_EXCEPTION_IO_CORRUPT;
        std::vector<unsigned char> data(static_cast<size_t>(size));
        if (input->seek(offset, SEEK_SET) || input->read(data.data(), 1, size_t(size)) != int(size))
            throw LIBRAW_EXCEPTION_IO_EOF;
        auto& s = imgdata.sizes;
        if (high_efficiency(data.data(), data.size(), imgdata.rawdata.raw_image, s.raw_width,
                            s.raw_height, s.raw_pitch / 2, high_efficiency_error))
            throw LIBRAW_EXCEPTION_DECODE_RAW;
    }
    void scale_colors_loop(float mul[4]) override {
        const size_t n = size_t(imgdata.sizes.iwidth) * imgdata.sizes.iheight;
        double bound = 1;
        for (size_t i=0; i<n; ++i)
            for (int c=0; c<4; ++c)
                bound = std::max(bound, double(imgdata.image[i][c]) * mul[c]);
        scale_factor = float(std::min(1.0, 60000.0 / bound));
        float reduced[4];
        for (int c=0;c<4;++c) reduced[c] = mul[c]*scale_factor;
        decode_gain = 1.f / (65535.f * scale_factor * imgdata.color.pre_mul[1]);
        LibRaw::scale_colors_loop(reduced);
        for (size_t i=0;i<n;++i)
            for(int c=0;c<4;++c) scale_clipped += imgdata.image[i][c] == 65535;
    }
};
// The black level an optical-black border says, when LibRaw's is far below it (LibRaw
// reads the EOS R6 Mark III's maker notes at the wrong offsets and gets 0 plus small
// per-channel values; Adobe and the border say 512). Returns -1 to keep LibRaw's: too
// few values, a border that is not dark and even (image, not masked pixels), or a black
// (the mean over the four channels) LibRaw reads at a quarter of the border or more:
// some bodies' borders sit above their true black (Pentax K-70: 130 against 64).
extern "C" int ora_masked_black(const unsigned short* border, size_t n, unsigned black, unsigned maximum) {
    if(n < 1000) return -1;
    std::vector<unsigned short> v(border, border+n);
    auto at = [&](size_t i) { std::nth_element(v.begin(), v.begin()+i, v.end()); return unsigned(v[i]); };
    const unsigned p10=at(n/10), p50=at(n/2), p90=at(n*9/10);
    if(p50 < 64 || p50 > maximum/8 || p90-p10 > std::max(64u, p50/4)) return -1;
    if(black*4 >= p50) return -1;
    return int(p50);
}
// Replaces LibRaw's black with the left optical-black border's when ora_masked_black
// finds it wrong. dcraw_process and ora_cfa_copy both read rawdata.color afterwards.
static void correct_black(LibRaw& raw) {
    auto& d=raw.imgdata;
    auto& col=d.rawdata.color;
    const unsigned left=d.sizes.left_margin;
    if(!d.rawdata.raw_image || left < 16) return;
    unsigned black=col.black + (col.cblack[0]+col.cblack[1]+col.cblack[2]+col.cblack[3])/4;
    const unsigned pattern=col.cblack[4]*col.cblack[5];
    if(pattern) {
        unsigned sum=0;
        for(unsigned i=0;i<pattern && i<LIBRAW_CBLACK_SIZE-6;++i) sum+=col.cblack[6+i];
        black+=sum/pattern;
    }
    const size_t pitch=d.sizes.raw_pitch/2;
    std::vector<unsigned short> border;
    // Skip the outermost columns and those next to the image, which can be lit.
    for(unsigned y=d.sizes.top_margin; y<d.sizes.top_margin+d.sizes.height && y<d.sizes.raw_height; y+=3)
        for(unsigned x=4; x+8<left; ++x) border.push_back(d.rawdata.raw_image[y*pitch+x]);
    const int masked=ora_masked_black(border.data(), border.size(), black, col.maximum);
    if(masked < 0) return;
    col.black=unsigned(masked);
    std::fill(std::begin(col.cblack), std::end(col.cblack), 0u);
}
static void message(char* err, const char* text) { std::snprintf(err, 512, "%s", text); }
// Nikon's High Efficiency raws are JPEG XS, which LibRaw cannot decode; RAWmakase
// decodes them itself. LibRaw knows them only on the bodies it lists (the Z 9, Z 8,
// Z f and Z6_3); on others, such as the Z5_2, it would read them as lossless
// compressed: noise, with a warning and no error. So any NEF whose data starts with
// JPEG XS's SOC and CAP markers is one.
static bool nikon_high_efficiency(Raw& raw) {
    const char* decoder=raw.unpack_function_name();
    // None after a failed unpack, which recycles the decoder.
    if(!decoder) return false;
    if(!std::strcmp(decoder,"nikon_he_load_raw()")) return true;
    if(std::strcmp(decoder,"nikon_load_raw()")) return false;
    auto* internal=raw.get_internal_data_pointer();
    auto* input=internal->internal_data.input;
    const unsigned char jpeg_xs[4]={0xff,0x10,0xff,0x50};
    unsigned char head[4];
    // unpack() seeks to the data itself.
    return input && !input->seek(internal->unpacker_data.data_offset,SEEK_SET)
        && input->read(head,1,4)==4 && !std::memcmp(head,jpeg_xs,4);
}
struct Handle {
    Raw raw;
    Cancel cancel = nullptr;
    void* context = nullptr;
    HighEfficiency high_efficiency = nullptr;
    // Highlight Tone Priority as Canon's custom functions record it, -1 without.
    int custom_htp = -1;
    // The per-channel blacks (LibRaw's order: red, green, blue, second green) and
    // the specular white of a ColorData12 record LibRaw reads as ColorData11; 0
    // without one.
    unsigned color_data_black[4] = {};
    unsigned color_data_white = 0;
    // LibRaw rejects a second unpack(); the CFA path may unpack before falling back.
    bool unpacked = false;
    // Fills err when it fails.
    int unpack(char* err) {
        if(unpacked) return 0;
        if(nikon_high_efficiency(raw)) raw.decode_high_efficiency(high_efficiency);
        int rc=raw.unpack();
        if(rc) message(err,raw.high_efficiency_error[0] ? raw.high_efficiency_error : libraw_strerror(rc));
        unpacked = rc==0;
        if(unpacked) { color_data_black_level(); correct_black(raw); }
        return rc;
    }
    // Replaces the black LibRaw read from the wrong offsets of a ColorData12
    // record (canon_maker_notes) with the record's own. Canon records 14-bit
    // levels; LibRaw scales them down to the data's depth, as it does for its own.
    void color_data_black_level() {
        auto& d=raw.imgdata;
        auto& col=d.rawdata.color;
        if(!color_data_black[0] || d.idata.maker_index!=LIBRAW_CAMERAMAKER_Canon
           || d.idata.dng_version || !d.rawdata.raw_image) return;
        unsigned shift=0;
        const unsigned bps=d.color.raw_bps;
        if(bps>0 && bps<14 && color_data_white > (1u<<bps)-1) shift=14-bps;
        col.black=0;
        std::fill(std::begin(col.cblack), std::end(col.cblack), 0u);
        for(int c=0;c<4;++c) col.cblack[c]=color_data_black[c]>>shift;
    }
};
// Canon's CustomFunctions2 maker note (0x0099), where the bodies from the 40D to the
// 600D record Highlight Tone Priority (record 0x0203). LibRaw reads it only from
// CameraInfo, which the 60D's lacks and which says Off on the 550D's when the custom
// function is on; Camera Raw follows the custom function. The note is groups of
// records: a 2-byte size, 2 bytes, the group count; per group its number, length
// (from the length field on) and record count; per record its tag, value count and
// 4-byte values.
static void canon_custom_functions(void* p, int tag, int type, int len, unsigned order,
                                   void* stream) {
    if (tag != 0x0099 || (type != 4 && type != 7) || len <= 0) return;
    const size_t size = size_t(len) * (type == 4 ? 4 : 1);
    if (size < 8 || size > 4096) return;
    std::vector<unsigned char> b(size);
    auto in = static_cast<LibRaw_abstract_datastream*>(stream);
    if (in->read(b.data(), 1, int(size)) != int(size)) return;
    auto u32 = [&](size_t at) {
        unsigned v = 0;
        for (int i = 0; i < 4; ++i)
            v |= unsigned(b[at + (order == 0x4949 ? i : 3 - i)]) << (8 * i);
        return v;
    };
    size_t at = 8;
    for (unsigned g = u32(4); g-- && at + 12 <= size;) {
        const size_t end = std::min<size_t>(size, at + 4 + size_t(u32(at + 4)));
        if (end <= at + 12) return;
        size_t r = at + 12;
        for (unsigned n = u32(at + 8); n-- && r + 8 <= end;) {
            const unsigned record = u32(r), values = u32(r + 4);
            if (record == 0x0203 && values >= 1 && r + 12 <= end) {
                static_cast<Handle*>(p)->custom_htp = int(u32(r + 8));
                return;
            }
            if (values > (end - r) / 4) return;
            r += 8 + size_t(values) * 4;
        }
        at = end;
    }
}
// Canon's ColorData (0x4001). The EOS R50 V and PowerShot V1 (version 65), and
// the EOS R6 Mark III and C50 (66), write the ColorData12 layout of the R1 and
// R5 Mark II in a record of 3778 words, ColorData11's length, which LibRaw reads
// with ColorData11's offsets: blacks of 0 to 150 instead of 512 or 2048, a pink
// cast where the masked border cannot correct it. ColorData12 keeps the blacks
// (red, green, green, blue) at 0x290 and Normal and Specular White at 0x294.
static void canon_color_data(void* p, int type, int len, unsigned order, void* stream) {
    if (type != 3 || len != 3778) return;
    std::vector<unsigned char> b(size_t(len) * 2);
    auto in = static_cast<LibRaw_abstract_datastream*>(stream);
    if (in->read(b.data(), 1, int(b.size())) != int(b.size())) return;
    auto u16 = [&](size_t word) {
        const unsigned lo = b[2 * word], hi = b[2 * word + 1];
        return order == 0x4949 ? lo | hi << 8 : hi | lo << 8;
    };
    const unsigned version = u16(0);
    if (version != 65 && version != 66) return;
    const unsigned black[4] = {u16(0x290), u16(0x291), u16(0x292), u16(0x293)};
    const unsigned white = u16(0x295);
    for (unsigned v : black)
        if (v == 0 || v >= white) return;
    auto h = static_cast<Handle*>(p);
    const int rgbg[4] = {0, 1, 3, 2};
    for (int c = 0; c < 4; ++c) h->color_data_black[rgbg[c]] = black[c];
    h->color_data_white = white;
}
static void canon_maker_notes(void* p, int tag, int type, int len, unsigned order,
                              void* stream, INT64) {
    if (tag == 0x0099) canon_custom_functions(p, tag, type, len, order, stream);
    else if (tag == 0x4001) canon_color_data(p, type, len, order, stream);
}
static int progress(void* p, LibRaw_progress, int, int) {
    auto h = static_cast<Handle*>(p);
    return h->cancel && h->cancel(h->context);
}
// Paths arrive as UTF-8. Windows' narrow file API uses the ANSI code page, so
// they are widened for LibRaw's wide-character open there.
static int open_path(Raw& raw, const char* path) {
#ifdef _WIN32
    int n = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, path, -1, nullptr, 0);
    if (n <= 0) return LIBRAW_IO_ERROR;
    std::wstring wide(size_t(n), L'\0');
    MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, path, -1, wide.data(), n);
    return raw.open_file(wide.c_str());
#else
    return raw.open_file(path);
#endif
}
extern "C" {
const char* ora_version() { return LibRaw::version(); }
// Caps the calling thread's OpenMP regions, its LibRaw decodes among them, at two
// threads (one on a single core), so they take only cores the foreground leaves
// idle; the thread's scheduling priority is set on the Rust side.
void ora_background_thread() {
#ifdef _OPENMP
    // The OpenMP setting is the thread's own; other threads keep every core.
    omp_set_num_threads(std::min(2, std::max(1, omp_get_num_procs())));
#endif
}
// So the Rust side can check its mirror of Metadata has the same layout.
unsigned ora_metadata_size() { return sizeof(Metadata); }
void* ora_open(const char* path, Metadata* m, HighEfficiency high_efficiency, char* err) {
    try {
        auto h = std::make_unique<Handle>();
        h->high_efficiency = high_efficiency;
        h->raw.set_makernotes_handler(canon_maker_notes, h.get());
        int rc = open_path(h->raw, path);
        if (rc) { message(err, libraw_strerror(rc)); return nullptr; }
        auto& d = h->raw.imgdata;
        // Canon's sRAW and mRAW (and Nikon's sRAW) are stored as YCbCr, not a mosaic;
        // LibRaw converts them to camera RGB, so they take ora_develop's path.
        if (d.idata.colors != 3 || (!d.idata.filters && !h->raw.is_sraw())) {
            message(err, "Only three-color Bayer, X-Trans and small RAW files are supported"); return nullptr;
        }
        *m = {};
        m->width=d.sizes.width; m->height=d.sizes.height;
        m->raw_width=d.sizes.raw_width; m->raw_height=d.sizes.raw_height;
        m->left_margin=d.sizes.left_margin; m->top_margin=d.sizes.top_margin;
        // The frame Adobe calls the default crop, and the in-camera aspect-ratio crop
        // inside it (Adobe's default user crop), both in raw coordinates. LibRaw
        // puts the aspect crop in raw_inset_crops[1], except for Canon, whose
        // AspectInfo it applies to raw_inset_crops[0]; the sensor's default crop
        // is then SensorInfo's.
        auto frame=d.sizes.raw_inset_crops[0], aspect=d.sizes.raw_inset_crops[1];
        const auto& canon=d.makernotes.canon.DefaultCropAbsolute;
        if (d.idata.maker_index==LIBRAW_CAMERAMAKER_Canon && !d.idata.dng_version
            && canon.l>=0 && canon.t>=0 && canon.r>canon.l && canon.b>canon.t) {
            aspect=frame;
            frame.cleft=ushort(canon.l); frame.ctop=ushort(canon.t);
            frame.cwidth=ushort(canon.r-canon.l+1); frame.cheight=ushort(canon.b-canon.t+1);
        }
        m->crop_width=frame.cwidth; m->crop_height=frame.cheight;
        m->crop_left=frame.cleft; m->crop_top=frame.ctop;
        m->aspect_left=aspect.cleft; m->aspect_top=aspect.ctop;
        m->aspect_width=aspect.cwidth; m->aspect_height=aspect.cheight;
        m->flip=d.sizes.flip; m->xtrans=d.idata.filters==9;
        m->dng_version=d.idata.dng_version;
        m->sraw=h->raw.is_sraw();
        m->fuji_dynamic_range=d.makernotes.fuji.DevelopmentDynamicRange;
        m->iso=d.other.iso_speed; m->shutter=d.other.shutter;
        m->aperture=d.other.aperture; m->focal=d.other.focal_len;
        m->focal_35mm=d.lens.FocalLengthIn35mmFormat;
        m->highlight_tone_priority=d.makernotes.canon.HighlightTonePriority;
        if (d.idata.maker_index==LIBRAW_CAMERAMAKER_Canon && !m->highlight_tone_priority
            && h->custom_htp==1) m->highlight_tone_priority=1;
        m->fuji_exposure_shift=d.makernotes.fuji.ExpoMidPointShift;
        if (!d.idata.dng_version) {
            for (int c=0; c<3; ++c) {
                m->preset_daylight[c] = d.color.WB_Coeffs[LIBRAW_WBI_Daylight][c];
                m->preset_shade[c] = d.color.WB_Coeffs[LIBRAW_WBI_Shade][c];
                m->preset_d65[c] = d.color.WB_Coeffs[LIBRAW_WBI_D65][c];
                m->preset_fine_weather[c] = d.color.WB_Coeffs[LIBRAW_WBI_FineWeather][c];
            }
        }
        for(int c=0;c<3;++c) {
            m->daylight_wb[c] = d.color.pre_mul[c];
            m->wb[c] = d.color.cam_mul[c] > 0 ? d.color.cam_mul[c] : d.color.pre_mul[c];
            for(int j=0;j<3;++j) {
                m->matrix[c*3+j]=d.color.rgb_cam[c][j];
                m->cam_xyz[c*3+j]=d.color.cam_xyz[c][j];
            }
        }
        float green = m->wb[1] > 0 ? m->wb[1] : 1;
        for(float& v : m->wb) v /= green;
        green=m->daylight_wb[1]>0?m->daylight_wb[1]:1;
        for(float& v:m->daylight_wb) v=std::max(0.001f,v/green);
        std::snprintf(m->make,64,"%s",d.idata.make);
        std::snprintf(m->model,64,"%s",d.idata.model);
        std::snprintf(m->lens,128,"%s",d.lens.Lens[0] ? d.lens.Lens : d.lens.makernotes.Lens);
        m->lens_id=d.lens.nikon.LensIDNumber;
        // Pentax's LensType, its two bytes as one number (4 252 is 0x4fc), as
        // Adobe's Pentax profiles write it: the only lens identity these bodies record.
        const auto pentax=d.lens.makernotes.LensID;
        if (d.idata.maker_index==LIBRAW_CAMERAMAKER_Pentax && pentax!=LIBRAW_LENS_NOT_SET
            && pentax<=0xffff) m->lens_id=unsigned(pentax);
        std::snprintf(m->normalized_make,64,"%s",d.idata.normalized_make);
        std::snprintf(m->normalized_model,64,"%s",d.idata.normalized_model);
        return h.release();
    } catch(const std::exception& e) { message(err,e.what()); return nullptr; }
    catch(...) { message(err,"Native RAW open failed"); return nullptr; }
}
void ora_close(void* h) { delete static_cast<Handle*>(h); }
int ora_develop(void* ptr, int fast, Cancel cancel, void* context,
                unsigned* w, unsigned* h, float* gain, float* scale, unsigned* clipped, char* err) {
    try {
        auto& handle=*static_cast<Handle*>(ptr); auto& raw=handle.raw;
        handle.cancel=cancel; handle.context=context;
        raw.set_progress_handler(progress,&handle);
        auto& p=raw.imgdata.params;
        p.use_camera_wb=1; p.use_auto_wb=0; p.no_auto_bright=1;
        p.adjust_maximum_thr=0; p.highlight=1; p.output_color=0;
        p.output_bps=16; p.gamm[0]=p.gamm[1]=1;
        p.user_flip=0; p.use_fuji_rotate=0;
        // AHD for Bayer. For X-Trans, quality 2 selects 1-pass Markesteijn (darktable's
        // default), about three times faster than the 3-pass variant.
        p.half_size=fast; p.user_qual=fast ? 0 : (raw.imgdata.idata.filters==9 ? 2 : 3);
        int rc=handle.unpack(err);
        if(rc) return rc;
        rc=raw.dcraw_process();
        if(rc) { message(err,libraw_strerror(rc)); return rc; }
        *w=raw.imgdata.sizes.width; *h=raw.imgdata.sizes.height;
        *gain=raw.decode_gain; *scale=raw.scale_factor; *clipped=raw.scale_clipped;
        return 0;
    } catch(const std::exception& e) { message(err,e.what()); return -1; }
    catch(...) { message(err,"Native RAW development failed"); return -1; }
}
// Unpacked sensor data for RAWmakase's own demosaic. Returns 0 with the visible size and
// a 48×48 colour pattern (covers 2-, 6- and 16-pixel periods) (0 red, 1 green, 2 blue) for single-channel Bayer or X-Trans
// data; nonzero means the caller should use ora_develop instead.
int ora_cfa_open(void* ptr, unsigned* w, unsigned* h, unsigned char* pattern, char* err) {
    try {
        auto& handle=*static_cast<Handle*>(ptr); auto& raw=handle.raw;
        int rc=handle.unpack(err);
        if(rc) return rc;
        auto& d=raw.imgdata;
        if(!d.rawdata.raw_image || d.idata.colors!=3 || !d.idata.filters || d.rawdata.color.maximum<=d.rawdata.color.black) {
            message(err,"Not single-channel CFA data"); return 1;
        }
        if(size_t(d.sizes.top_margin)+d.sizes.height>d.sizes.raw_height
           || size_t(d.sizes.left_margin)+d.sizes.width>d.sizes.raw_width) {
            message(err,"Visible area outside raw data"); return 1;
        }
        *w=d.sizes.width; *h=d.sizes.height;
        for(int r=0;r<48;++r) for(int c=0;c<48;++c) {
            int v=raw.COLOR(r,c);
            pattern[r*48+c]=(unsigned char)(v==3 ? 1 : v);
        }
        return 0;
    } catch(const std::exception& e) { message(err,e.what()); return -1; }
    catch(...) { message(err,"Native RAW unpack failed"); return -1; }
}
// Fills width×height values: (raw − black) / (white − black) per pixel, not white balanced.
void ora_cfa_copy(void* ptr, float* out) {
    auto& raw=static_cast<Handle*>(ptr)->raw;
    auto& d=raw.imgdata;
    const auto& col=d.rawdata.color;
    const unsigned w=d.sizes.width, h=d.sizes.height;
    const size_t pitch=d.sizes.raw_pitch/2;
    const unsigned cr=col.cblack[4], cc=col.cblack[5];
    const bool pat = cr>0 && cc>0 && size_t(cr)*cc<=LIBRAW_CBLACK_SIZE-6;
    #pragma omp parallel for schedule(static)
    for(int row=0; row<(int)h; ++row) {
        const ushort* src=d.rawdata.raw_image+size_t(row+d.sizes.top_margin)*pitch+d.sizes.left_margin;
        float* dst=out+size_t(row)*w;
        for(unsigned c=0;c<w;++c) {
            int k=raw.COLOR(row,c);
            float black=float(col.black+col.cblack[k<4?k:1]);
            if(pat) black+=float(col.cblack[6+(row%cr)*cc+(c%cc)]);
            float white=float(col.maximum);
            dst[c]=std::max(0.f,(float(src[c])-black)/std::max(1.f,white-black));
        }
    }
}
unsigned ora_sensor_size() { return sizeof(Sensor); }
// Unpacks as development does (the black correction included) and describes the
// sensor data: one channel for a mosaic, three or four for small RAW files.
// The colour pattern is COLOR's over the visible area for one period of LibRaw's
// filters: 16 by 2, 16 by 16 for filters 1 (Leaf) and Super CCD, 6 by 6 for X-Trans.
int ora_sensor(void* ptr, Sensor* s, char* err) {
    try {
        auto& handle=*static_cast<Handle*>(ptr); auto& raw=handle.raw;
        int rc=handle.unpack(err);
        if(rc) return rc;
        auto& d=raw.imgdata;
        const auto& col=d.rawdata.color;
        *s = {};
        s->width=d.sizes.raw_width; s->height=d.sizes.raw_height;
        if(d.rawdata.raw_image) { s->data=d.rawdata.raw_image; s->channels=1; }
        else if(d.rawdata.color3_image) { s->data=d.rawdata.color3_image[0]; s->channels=3; }
        else if(d.rawdata.color4_image) { s->data=d.rawdata.color4_image[0]; s->channels=4; }
        else { message(err,"No integer sensor data"); return -1; }
        s->pitch=d.sizes.raw_pitch/2;
        s->left=d.sizes.left_margin; s->top=d.sizes.top_margin;
        // Fujifilm's Super CCD is stored rotated by 45°: development reads
        // fuji_width columns (twice that in one layout) of the rows between the
        // top and bottom margins (raw2image). Other sensors, the visible area.
        auto* internal=raw.get_internal_data_pointer();
        if(const unsigned fuji=internal->internal_output_params.fuji_width) {
            const unsigned cols=fuji << (internal->unpacker_data.fuji_layout ? 0 : 1);
            const unsigned rw=d.sizes.raw_width, rh=d.sizes.raw_height;
            s->area_width=std::min(cols, rw-std::min(rw, unsigned(d.sizes.left_margin)));
            s->area_height=rh-std::min(rh, 2u*d.sizes.top_margin);
        } else {
            s->area_width=d.sizes.width; s->area_height=d.sizes.height;
        }
        s->black=col.cblack; s->base_black=col.black;
        const size_t pattern=size_t(col.cblack[4])*col.cblack[5];
        s->black_size=unsigned(6+std::min(pattern, size_t(LIBRAW_CBLACK_SIZE-6)));
        s->white=col.maximum;
        for(int c=0;c<4;++c) s->linear_white[c]=unsigned(col.linear_max[c]);
        const unsigned filters=d.idata.filters;
        if(filters) {
            // COLOR follows a Super CCD's rotation, which repeats across more columns.
            const bool wide=filters==1 || internal->internal_output_params.fuji_width;
            s->pattern_rows=filters==9 ? 6 : 16;
            s->pattern_cols=filters==9 ? 6 : wide ? 16 : 2;
            for(unsigned r=0;r<s->pattern_rows;++r) for(unsigned c=0;c<s->pattern_cols;++c)
                s->pattern[r*s->pattern_cols+c]=(unsigned char)raw.COLOR(int(r),int(c));
        }
        return 0;
    } catch(const std::exception& e) { message(err,e.what()); return -1; }
    catch(...) { message(err,"Native RAW unpack failed"); return -1; }
}
void ora_copy(void* ptr, float* out) {
    auto& r=static_cast<Handle*>(ptr)->raw;
    size_t n=size_t(r.imgdata.sizes.width)*r.imgdata.sizes.height;
    for(size_t i=0;i<n;++i) for(int c=0;c<3;++c)
        out[i*3+c]=r.imgdata.image[i][c]*r.decode_gain;
}
int ora_thumbnail(void* ptr, unsigned char** data, unsigned* size, char* err) {
    try {
        auto& r=static_cast<Handle*>(ptr)->raw;
        int rc=r.unpack_thumb();
        if(rc) { message(err,libraw_strerror(rc)); return rc; }
        if(r.imgdata.thumbnail.tformat!=LIBRAW_THUMBNAIL_JPEG) {
            message(err,"Embedded preview is not JPEG"); return -1;
        }
        *data=reinterpret_cast<unsigned char*>(r.imgdata.thumbnail.thumb);
        *size=r.imgdata.thumbnail.tlength;
        return 0;
    } catch(...) { message(err,"Embedded preview failed"); return -1; }
}
}
extern "C" int ora_scale_probe(float wb, float* error) {
    Raw r;
    r.imgdata.sizes.iwidth=16384; r.imgdata.sizes.iheight=1;
    r.imgdata.color.pre_mul[1]=1.f/wb;
    r.imgdata.image=static_cast<ushort(*)[4]>(calloc(16384,sizeof(ushort[4])));
    if(!r.imgdata.image) return -1;
    for(int i=0;i<16384;++i) for(int c=0;c<4;++c) r.imgdata.image[i][c]=i;
    float mul[4]={65535.f/16383.f,65535.f/16383.f/wb,65535.f/16383.f/wb,65535.f/16383.f/wb};
    r.scale_colors_loop(mul);
    *error=0;
    for(int i=0;i<16384;++i) {
        float value=r.imgdata.image[i][0]*r.decode_gain;
        *error=std::max(*error,std::abs(value-float(i)/16383.f*wb));
    }
    return r.scale_clipped;
}
