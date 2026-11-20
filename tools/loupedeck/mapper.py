#!/usr/bin/env python3
"""Interactive mapper: operate a control, name it, repeat. Saves mapping.json.

Usage: .venv/bin/python mapper.py
Type 'done' (or Ctrl-C) as a name/at the prompt to finish. Empty name = skip.
"""
import json
import os
import time

import rtmidi

OUT = "mapping.json"
QUIET = 0.7  # seconds of silence that ends one gesture


def sig(msg):
    # control identity: message kind + channel + first data byte
    # (for pitch bend, kind+channel only, since data bytes are the value)
    kind, ch = msg[0] & 0xF0, msg[0] & 0x0F
    return (kind, ch) if kind == 0xE0 else (kind, ch, msg[1] if len(msg) > 1 else None)


def key(s):
    return "/".join(str(x) for x in s)


def main():
    mi = rtmidi.MidiIn()
    ports = mi.get_ports()
    idx = next((i for i, p in enumerate(ports) if "Loupedeck" in p), None)
    if idx is None:
        raise SystemExit(f"Loupedeck port not found: {ports}")
    mi.open_port(idx)
    mi.ignore_types(sysex=False, timing=False, active_sense=False)

    mapping = json.load(open(OUT)) if os.path.exists(OUT) else {}
    known = {k: v["name"] for k, v in mapping.items()}
    print(f"Loaded {len(mapping)} existing mappings. Operate a control (Ctrl-C to finish).\n")

    try:
        while True:
            while (m := mi.get_message()) is None:
                time.sleep(0.002)
            gesture, last = [m[0]], time.time()
            while time.time() - last < QUIET:
                if (m := mi.get_message()) is not None:
                    gesture.append(m[0])
                    last = time.time()
                else:
                    time.sleep(0.002)

            # group by control; a gesture may touch >1 signature (e.g. touch + press)
            groups = {}
            for msg in gesture:
                groups.setdefault(sig(msg), []).append(msg)
            for s, msgs in groups.items():
                k = key(s)
                vals = sorted({tuple(x[1:]) for x in msgs})
                if k in known:
                    print(f"  [{k}] already mapped as '{known[k]}' ({len(msgs)} msgs)")
                    continue
                shown = ", ".join(str(list(v)) for v in vals[:8])
                print(f"  NEW [{k}] type={hex(s[0])} ch={s[1]+1} {len(msgs)} msgs, values: {shown}")
                name = input("  name for this control (blank=skip, 'done'=quit)> ").strip()
                if name.lower() == "done":
                    raise KeyboardInterrupt
                if name:
                    known[k] = name
                    mapping[k] = {"name": name, "status": s[0], "channel": s[1] + 1,
                                  "data1": s[2] if len(s) > 2 else None,
                                  "values_seen": [list(v) for v in vals]}
                    json.dump(mapping, open(OUT, "w"), indent=2)
            print()
    except KeyboardInterrupt:
        pass
    print(f"\nSaved {len(mapping)} controls to {OUT}")


if __name__ == "__main__":
    main()
