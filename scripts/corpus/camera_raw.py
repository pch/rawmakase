"""Camera Raw renders through Photoshop, at the comparison boundary of
docs/scene-tone-stage.md: 16-bit ProPhoto RGB (gamma 1.8) TIFFs, at the source's
size or reduced by area to a long edge.

`render(jobs)` takes dicts with `source` (a RAW or DNG), `settings` (crs: attributes
without the prefix), `out` (TIFF path) and optionally `curves` ({name: [points]}),
`look` ({'file': look profile XMP, 'amount': 0-2, 1 is 100%}), `edge`, or `xmp_text`, a
complete sidecar used instead of the settings. Each job opens a fresh copy of the source, since Camera Raw writes
settings into DNGs and sidecars next to the file it opens; originals are never
opened. Jobs whose output exists are skipped. Photoshop must have no open
documents (another session may be using it).
"""
import json
import plistlib
import re
import subprocess
import tempfile
from pathlib import Path

import numpy as np

PHOTOSHOP = 'Adobe Photoshop 2026'
CAMERA_RAW_PLIST = Path('/Library/Application Support/Adobe/Plug-Ins/CC/File Formats/'
                        'Camera Raw.plugin/Contents/Info.plist')
# Settings every reference starts from: Process Version 2012 (15.4), no lens or
# detail processing that would blur the comparison, As Shot white balance.
BASE = {
    'ProcessVersion': '15.4', 'WhiteBalance': 'As Shot', 'LensProfileEnable': '0', 'AutoLateralCA': '0',
    'Sharpness': '0', 'LuminanceSmoothing': '0', 'ColorNoiseReduction': '0',
    'Exposure2012': '0', 'Contrast2012': '0', 'Highlights2012': '0', 'Shadows2012': '0',
    'Whites2012': '0', 'Blacks2012': '0', 'Texture': '0', 'Clarity2012': '0', 'Dehaze': '0',
    'Vibrance': '0', 'Saturation': '0', 'PostCropVignetteAmount': '0', 'GrainAmount': '0',
}

TEMPLATE = r'''#target photoshop
app.displayDialogs = DialogModes.NO;
var jobs = %(jobs)s;
var failed = [];
for (var i = 0; i < jobs.length; i++) {
  var j = jobs[i];
  var out = new File(j.out);
  if (out.exists) continue;
  new Folder(out.parent).create();
  var raw = new File(j.copy);
  if (raw.exists) raw.remove();
  new File(j.source).copy(j.copy);
  var x = new File(j.xmp);
  x.encoding = "UTF-8"; x.open("w"); x.write(j.settings); x.close();
  var o = new CameraRAWOpenOptions();
  o.bitsPerChannel = BitsPerChannelType.SIXTEEN;
  o.colorSpace = ColorSpaceType.PROPHOTORGB;
  var doc;
  try { doc = app.open(raw, o); } catch (e) { failed.push(j.out + ": " + e); continue; }
  try {
    if (doc.bitsPerChannel != BitsPerChannelType.SIXTEEN) doc.bitsPerChannel = BitsPerChannelType.SIXTEEN;
    if (doc.colorProfileName != "ProPhoto RGB") throw new Error("opened in " + doc.colorProfileName);
    var w = doc.width.as("px"), h = doc.height.as("px"), s = j.edge / Math.max(w, h);
    if (j.edge > 0 && s < 1) doc.resizeImage(UnitValue(Math.round(w * s), "px"), UnitValue(Math.round(h * s), "px"), null, ResampleMethod.BICUBIC);
    var t = new TiffSaveOptions(); t.imageCompression = TIFFEncoding.NONE; t.embedColorProfile = true;
    doc.saveAs(out, t, true);
  } catch (e) {
    failed.push(j.out + ": " + e);
  } finally {
    doc.close(SaveOptions.DONOTSAVECHANGES);
  }
}
failed.join("\n");
'''


def look_element(path, amount):
    """A look profile as Lightroom writes it into a sidecar: the `Look` element with the
    profile's parameters, and its tables as top-level `Table_` attributes."""
    text = Path(path).read_text()
    attributes = dict(re.findall(r'crs:(\w+)="([^"]*)"', text))
    name = re.search(r'xml:lang="x-default">([^<]*)<', text).group(1)
    meta = ('PresetType', 'Cluster', 'UUID', 'CameraModelRestriction', 'Copyright', 'ContactInfo')
    parameters = ' '.join(f'crs:{k}="{v}"' for k, v in attributes.items()
                          if k not in meta and not k.startswith(('Supports', 'Table_')))
    curves = ''.join(re.findall(r'(<crs:ToneCurvePV2012\w*>.*?</crs:ToneCurvePV2012\w*>)', text, re.S))
    element = (f'<crs:Look><rdf:Description crs:Name="{name}" crs:Amount="{amount}"'
               f' crs:UUID="{attributes["UUID"]}" crs:SupportsAmount="{attributes["SupportsAmount"].lower()}"'
               f' crs:SupportsMonochrome="{attributes["SupportsMonochrome"].lower()}" crs:SupportsOutputReferred="false">'
               f'<crs:Parameters><rdf:Description {parameters}>{curves}</rdf:Description></crs:Parameters>'
               '</rdf:Description></crs:Look>')
    return element, {k: v for k, v in attributes.items() if k.startswith('Table_')}


def xmp(settings, curves=None, look=None):
    element, tables = look_element(look['file'], look.get('amount', 1)) if look else ('', {})
    attributes = dict(BASE, **settings, **tables)
    text = ('<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">'
            '<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" '
            'crs:HasSettings="True"')
    for k in sorted(attributes):
        text += f' crs:{k}="{attributes[k]}"'
    text += '>' + element
    for name, points in sorted((curves or {}).items()):
        text += f'<crs:{name}><rdf:Seq>' + ''.join(f'<rdf:li>{p}</rdf:li>' for p in points) + f'</rdf:Seq></crs:{name}>'
    return text + '</rdf:Description></rdf:RDF></x:xmpmeta>'


def version():
    if not CAMERA_RAW_PLIST.exists():
        return '?'
    return plistlib.loads(CAMERA_RAW_PLIST.read_bytes()).get('CFBundleShortVersionString', '?').split()[0]


def idle():
    out = subprocess.run(['osascript', '-e', f'tell application "{PHOTOSHOP}" to count documents'],
                         capture_output=True, text=True)
    return out.returncode == 0 and out.stdout.strip() == '0'


def render(jobs, work=None, chunk=200):
    """Render `jobs`; returns the failures Photoshop reported."""
    todo = [j for j in jobs if not Path(j['out']).exists()]
    if not todo:
        return []
    if not idle():
        raise RuntimeError(f'{PHOTOSHOP} is not running or has documents open')
    work = Path(work or tempfile.mkdtemp(prefix='camera-raw-')).resolve()
    work.mkdir(parents=True, exist_ok=True)
    failures = []
    for start in range(0, len(todo), chunk):
        batch = []
        for i, j in enumerate(todo[start:start + chunk]):
            source = Path(j['source']).resolve()
            copy = work / f'job{i % 2}{source.suffix}'
            batch.append(dict(source=str(source), copy=str(copy), xmp=str(copy.with_suffix('.xmp')),
                              settings=j.get('xmp_text') or xmp(j['settings'], j.get('curves'), j.get('look')), out=str(Path(j['out']).resolve()),
                              edge=int(j.get('edge', 0))))
        script = work / 'render.jsx'
        script.write_text(TEMPLATE % dict(jobs=json.dumps(batch)))
        # Photoshop 2026 refuses to read document properties from a script while
        # another application is frontmost.
        result = subprocess.run(['osascript', '-e', f'with timeout of 360000 seconds\ntell application "{PHOTOSHOP}"\n'
                                 f'activate\ndo javascript file (POSIX file {json.dumps(str(script))})\nend tell\n'
                                 'end timeout'],
                                capture_output=True, text=True, check=True)
        failures += [line for line in result.stdout.strip().splitlines() if line]
    return failures


def read_linear(path):
    """A ProPhoto TIFF from `render` as linear ProPhoto RGB, float64 (height, width, 3)."""
    import tifffile
    a = tifffile.imread(str(path)).astype(np.float64) / 65535.
    return a[..., :3] ** 1.8
