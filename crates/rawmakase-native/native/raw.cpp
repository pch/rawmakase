#include <libraw/libraw.h>
#ifdef _WIN32
// After LibRaw, which includes winsock2.h ahead of windows.h as Windows requires.
#include <windows.h>
#include <string>
#elif defined(__linux__)
#include <cerrno>
#include <sys/resource.h>
#include <sys/syscall.h>
#include <unistd.h>
#endif
#include <lcms2.h>
#ifdef _OPENMP
#include <omp.h>
#endif
#include <algorithm>
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
    float sony_daylight_wb[3];
};
typedef int (*Cancel)(void*);
}

// LibRaw retains its black subtraction and demosaicing. Only the common scale
// is reduced when necessary; its inverse is retained across the integer boundary.
class Raw : public LibRaw {
public:
    float decode_gain = 1;
    float scale_factor = 1;
    unsigned scale_clipped = 0;
    explicit Raw() : LibRaw() {}
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
// Nikon's High Efficiency raws are JPEG XS, which LibRaw cannot decode. It knows them
// only on the bodies it lists (the Z 9, Z 8, Z f and Z6_3); on others, such as the
// Z5_2, it reads them as lossless compressed: noise, with a warning and no error.
// So any NEF whose data starts with JPEG XS's SOC and CAP markers is refused.
static bool nikon_high_efficiency(Raw& raw) {
    const char* decoder=raw.unpack_function_name();
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
    // LibRaw rejects a second unpack(); the CFA path may unpack before falling back.
    bool unpacked = false;
    // Fills err when it fails.
    int unpack(char* err) {
        if(unpacked) return 0;
        if(nikon_high_efficiency(raw)) {
            message(err,"Nikon High Efficiency NEF files are not supported yet");
            return LIBRAW_FILE_UNSUPPORTED;
        }
        int rc=raw.unpack();
        if(rc) message(err,libraw_strerror(rc));
        unpacked = rc==0;
        if(unpacked) correct_black(raw);
        return rc;
    }
};
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
// threads (one on a single core), and on Linux and Windows lowers its scheduling
// priority, so it takes only cores the foreground leaves idle.
//
// macOS keeps the priority: libomp shares one pool of workers across the process,
// so a team started by a lower-QoS thread could later run Develop's decodes.
void ora_background_thread() {
#ifdef _OPENMP
    // The OpenMP setting is the thread's own; other threads keep every core.
    omp_set_num_threads(std::min(2, std::max(1, omp_get_num_procs())));
#endif
#ifdef _WIN32
    // Below the process's class, whatever it is. MSVC builds have no OpenMP.
    SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
#elif defined(__linux__)
    // Ten steps nicer than the thread is now, at most 19: raising a nice value
    // needs no privilege. Linux applies it to the one thread named by its id, and
    // libgomp's team for this thread, started from it, inherits it.
    const auto tid=static_cast<id_t>(syscall(SYS_gettid));
    errno=0;
    const int nice=getpriority(PRIO_PROCESS, tid);
    if(errno==0 && nice<19) setpriority(PRIO_PROCESS, tid, std::min(19, nice+10));
#endif
}
// So the Rust side can check its mirror of Metadata has the same layout.
unsigned ora_metadata_size() { return sizeof(Metadata); }
void* ora_open(const char* path, Metadata* m, char* err) {
    try {
        auto h = std::make_unique<Handle>();
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
        m->crop_width=d.sizes.raw_inset_crops[0].cwidth;
        m->crop_height=d.sizes.raw_inset_crops[0].cheight;
        m->crop_left=d.sizes.raw_inset_crops[0].cleft; m->crop_top=d.sizes.raw_inset_crops[0].ctop;
        m->flip=d.sizes.flip; m->xtrans=d.idata.filters==9;
        m->fuji_dynamic_range=d.makernotes.fuji.DevelopmentDynamicRange;
        m->iso=d.other.iso_speed; m->shutter=d.other.shutter;
        m->aperture=d.other.aperture; m->focal=d.other.focal_len;
        m->focal_35mm=d.lens.FocalLengthIn35mmFormat;
        m->highlight_tone_priority=d.makernotes.canon.HighlightTonePriority;
        m->fuji_exposure_shift=d.makernotes.fuji.ExpoMidPointShift;
        if (d.idata.maker_index == LIBRAW_CAMERAMAKER_Sony && !d.idata.dng_version) {
            for (int c=0; c<3; ++c)
                m->sony_daylight_wb[c] = d.color.WB_Coeffs[LIBRAW_WBI_Daylight][c];
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
unsigned ora_srgb_profile(unsigned char* data, unsigned capacity) {
    auto p=cmsCreate_sRGBProfile(); if(!p) return 0;
    cmsUInt32Number size=capacity;
    bool ok=cmsSaveProfileToMem(p,data,&size); cmsCloseProfile(p);
    return ok?size:0;
}
int ora_display(const char* path, unsigned char* rgb, unsigned count) {
    auto src=cmsCreate_sRGBProfile(); auto dst=cmsOpenProfileFromFile(path,"r");
    if(!src || !dst) { if(src) cmsCloseProfile(src); if(dst) cmsCloseProfile(dst); return -1; }
    auto t=cmsCreateTransform(src,TYPE_RGB_8,dst,TYPE_RGB_8,INTENT_RELATIVE_COLORIMETRIC,cmsFLAGS_BLACKPOINTCOMPENSATION);
    if(t) { cmsDoTransform(t,rgb,rgb,count); cmsDeleteTransform(t); }
    cmsCloseProfile(src); cmsCloseProfile(dst); return t?0:-1;
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
