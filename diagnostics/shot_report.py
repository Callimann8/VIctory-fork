#!/usr/bin/env python3
"""Frame shot report: where one frame's GPU time goes, and what the frame is made of.

Usage: python shot_report.py <shot folder> [--top N] [--out report.md]

Reads the `events.txt` and `stamps.txt` a frame shot writes (`VICTORY_SHOT`, see
crates/victory-gpu/src/shot.rs) and prints a Markdown report:

* GPU time per command kind, per pipeline (the title's names), per shader, with the
  translation's form (`[form ...]`: wave64 programs split into two 32-lane halves, their
  group barriers) and whether the command reads buffers from the PC's memory over PCIe
  (`in PC`).
* Time the layer puts in front of commands (binds, uploads, clamps, state moves: the
  `NEXT` marks, `VICTORY_SHOT_ENDS`, on by default) and title barriers (the GPU drains
  there), apart from the commands' own time.
* How much of the frame's span the card is busy (lists' stamped spans joined).
* The frame's render targets and depth buffers by size and format, the most likely
  internal render resolution and output resolution, motion-vector-like targets
  (two-channel 16-bit), and candidates for the game's own upscale pass (commands that
  read textures at the internal size and write at a larger size).

Timestamps are the card's clock before each stamped line (bottom of the pipe), so a
command's time is from its stamp to the next stamp in the same list. Work that overlaps
is counted where it ends; idle time inside a list (waiting on a fence, a barrier
draining) lands on the line before it.

Standard library only.
"""

import argparse
import collections
import os
import re
import sys

WORK = ("dispatch", "draw", "indexed", "mesh", "indirect")
KIND_RE = re.compile(r"^(\S+)")
HASH_RE = re.compile(r"\| ([0-9a-f]{8,32}):")
FORM_RE = re.compile(r"\[form ([^\]]*)\]")
TARGET_RE = re.compile(r'"0x[0-9a-f]+ (\d+)x(\d+) f(\d+)"')
DEPTH_RE = re.compile(r'depth Some\("0x[0-9a-f]+ (\d+)x(\d+) f(\d+)"\)')
READ_TEX_RE = re.compile(r"(?<!uav texture )(?<![0-9a-zA-Z])(\d+)x(\d+) f(\d+) flags (0x[0-9a-f]+)")
UAV_TEX_RE = re.compile(r"uav texture (\d+)x(\d+) f(\d+)")
PLACED_RE = re.compile(r"placed uav=(\w+) dim \d+ (\d+)x(\d+) f(\d+)")

DEPTH_FORMATS = {19: "D32S8X24 (typeless)", 20: "D32_FLOAT_S8X24", 39: "R32 typeless", 40: "D32_FLOAT",
                 44: "R24G8 typeless", 45: "D24_UNORM_S8", 55: "D16_UNORM"}
MOTION_FORMATS = {34: "R16G16_FLOAT", 35: "R16G16_UNORM", 37: "R16G16_SNORM", 38: "R16G16_SINT"}


def load(folder):
    freq = None
    ticks = {}
    with open(os.path.join(folder, "stamps.txt"), encoding="utf-8", errors="replace") as f:
        for line in f:
            a, _, b = line.rstrip("\n").partition("\t")
            if a == "frequency":
                freq = int(b)
            elif a and b:
                ticks[int(a)] = int(b)
    events = []
    with open(os.path.join(folder, "events.txt"), encoding="utf-8", errors="replace") as f:
        for line in f:
            parts = line.rstrip("\n").split("\t", 4)
            if len(parts) < 5:
                continue
            seq, lst, thread, stamp, text = parts
            stamp = int(stamp)
            events.append({"seq": int(seq), "list": lst, "thread": thread,
                           "stamp": ticks.get(stamp) if stamp >= 0 else None, "text": text})
    return freq, events


def kind_of(text):
    m = KIND_RE.match(text)
    k = m.group(1) if m else "?"
    if text.startswith("NEXT"):
        return "layer before command"
    if text.startswith("TITLE BARRIER"):
        return "title barrier"
    if k in WORK or k.startswith("indirect"):
        return k
    return " ".join(text.split()[:2])[:40]


def name_of(text):
    """The pipeline's name: the word after the command and its counts."""
    w = text.split()
    if len(w) >= 3 and (w[0] in WORK or w[0].startswith("indirect")):
        return w[2]
    return "-"


def timed(events, freq):
    """Each stamped event with its time (ms) to the next stamped event of its list."""
    by_list = collections.defaultdict(list)
    for e in events:
        if e["stamp"] is not None:
            by_list[e["list"]].append(e)
    spans = []
    for lst, evs in by_list.items():
        evs.sort(key=lambda e: e["seq"])
        for a, b in zip(evs, evs[1:]):
            d = b["stamp"] - a["stamp"]
            # (a stamp a list never wrote reads as garbage: leave out what is not plausible)
            if 0 <= d < freq:  # under a second
                a["ms"] = d * 1000.0 / freq
        if evs:
            spans.append((evs[0]["stamp"], evs[-1]["stamp"]))
    return spans


def busy(spans, freq):
    if not spans:
        return 0.0, 0.0
    spans = sorted(spans)
    total = 0
    cur_a, cur_b = spans[0]
    for a, b in spans[1:]:
        if a > cur_b:
            total += cur_b - cur_a
            cur_a, cur_b = a, b
        else:
            cur_b = max(cur_b, b)
    total += cur_b - cur_a
    whole = max(b for _, b in spans) - min(a for a, _ in spans)
    return total * 1000.0 / freq, whole * 1000.0 / freq


def table(rows, header):
    out = ["| " + " | ".join(header) + " |", "|" + "---|" * len(header)]
    for r in rows:
        out.append("| " + " | ".join(str(x) for x in r) + " |")
    return "\n".join(out)


def report(folder, top):
    freq, events = load(folder)
    if not freq:
        return "no timestamps in this shot (stamps.txt has no frequency)\n"
    spans = timed(events, freq)
    busy_ms, whole_ms = busy(spans, freq)
    out = [f"# Frame shot report: {os.path.basename(os.path.normpath(folder))}", ""]
    stamped = [e for e in events if "ms" in e]
    total = sum(e["ms"] for e in stamped)
    out.append(f"Lines {len(events)}, timed {len(stamped)}. Sum of timed lines {total:.1f} ms; "
               f"card busy (lists' spans joined) {busy_ms:.1f} ms of a {whole_ms:.1f} ms span "
               f"({100.0 * (1 - busy_ms / whole_ms) if whole_ms else 0:.0f}% with no list running).")
    out.append("")

    # Kinds
    kinds = collections.defaultdict(lambda: [0, 0.0])
    for e in stamped:
        k = kind_of(e["text"])
        kinds[k][0] += 1
        kinds[k][1] += e["ms"]
    rows = sorted(kinds.items(), key=lambda kv: -kv[1][1])[:top]
    out += ["## Time by kind of line", "",
            "`layer before command`: what the layer records in front of a title command (binds, "
            "uploads, indirect clamps, state moves); `title barrier`: the title's barriers, where the "
            "card drains (a stall, not work).", "",
            table([(k, n, f"{ms:.2f}") for k, (n, ms) in rows], ["kind", "lines", "ms"]), ""]

    # Commands: attach the layer time before each to it
    cmds = []
    pending = 0.0
    last_by_list = {}
    for e in sorted(events, key=lambda e: e["seq"]):
        t = e["text"]
        if t.startswith("NEXT"):
            last_by_list[e["list"]] = e.get("ms", 0.0)
            continue
        k = kind_of(t)
        if k in WORK or k.startswith("indirect"):
            cmds.append((e, last_by_list.pop(e["list"], 0.0)))
    _ = pending

    def agg(key):
        d = collections.defaultdict(lambda: {"n": 0, "ms": 0.0, "layer": 0.0, "pc": 0, "form": "", "name": ""})
        for e, layer in cmds:
            for k in key(e):
                r = d[k]
                r["n"] += 1
                r["ms"] += e.get("ms", 0.0)
                r["layer"] += layer
                r["pc"] += " in PC" in e["text"]
                f = FORM_RE.findall(e["text"])
                if f and not r["form"]:
                    r["form"] = "; ".join(f)[:90]
                if not r["name"]:
                    r["name"] = name_of(e["text"])
        return sorted(d.items(), key=lambda kv: -kv[1]["ms"])

    out += ["## Pipelines (the title's names) by GPU time", "",
            table([(k, r["n"], f"{r['ms']:.2f}", f"{r['layer']:.2f}", r["pc"], r["form"])
                   for k, r in agg(lambda e: [name_of(e["text"])])[:top]],
                  ["pipeline", "commands", "own ms", "layer ms before", "reading PC memory", "form"]), ""]
    out += ["## Shaders by GPU time", "",
            "A command counts for each of its shaders. `w64 ...`: a console wave64 compute program "
            "run as two 32-lane host waves, in the form named; `barriers lds=`: meetings of the two "
            "halves around LDS (fewer with `VICTORY_LDS_PRUNE=1`).", "",
            table([(k, r["name"], r["n"], f"{r['ms']:.2f}", r["form"])
                   for k, r in agg(lambda e: HASH_RE.findall(e["text"]) or ["-"])[:top]],
                  ["shader", "pipeline", "commands", "ms", "form"]), ""]

    # Longest single commands
    longest = sorted(cmds, key=lambda c: -c[0].get("ms", 0.0))[:top]
    out += ["## Longest single commands", "",
            table([(e["seq"], f"{e.get('ms', 0.0):.3f}", f"{layer:.3f}", e["text"][:160].replace("|", "/"))
                   for e, layer in longest], ["line", "own ms", "layer ms before", "text"]), ""]

    # Title barriers that cost the most
    bars = sorted((e for e in stamped if e["text"].startswith("TITLE BARRIER")), key=lambda e: -e["ms"])[:top]
    if bars:
        out += ["## Costliest title barriers (drains)", "",
                table([(e["seq"], f"{e['ms']:.3f}") for e in bars], ["line", "ms"]), ""]

    out.append(frame_layout(events))
    return "\n".join(out) + "\n"


def frame_layout(events):
    """Render targets, depth, motion vectors, internal/output resolution, upscale candidates."""
    targets = collections.Counter()
    depths = collections.Counter()
    uavs = collections.Counter()
    writes_by_size = collections.Counter()
    first_seen = {}
    cmd_list = []
    for e in sorted(events, key=lambda e: e["seq"]):
        t = e["text"]
        k = kind_of(t)
        if not (k in WORK or k.startswith("indirect")):
            continue
        written = []
        for w, h, f in TARGET_RE.findall(t.split(" depth ")[0] if " depth " in t else t):
            targets[(int(w), int(h), int(f))] += 1
            written.append((int(w), int(h), int(f)))
        for w, h, f in DEPTH_RE.findall(t):
            depths[(int(w), int(h), int(f))] += 1
            written.append((int(w), int(h), int(f)))
        for w, h, f in UAV_TEX_RE.findall(t):
            uavs[(int(w), int(h), int(f))] += 1
            written.append((int(w), int(h), int(f)))
        for _, w, h, f in PLACED_RE.findall(t):
            uavs[(int(w), int(h), int(f))] += 1
            written.append((int(w), int(h), int(f)))
        read = [(int(w), int(h), int(f)) for w, h, f, _ in READ_TEX_RE.findall(t)]
        # (target sizes also match the read pattern only with "flags": targets have none)
        for s in written:
            writes_by_size[s[:2]] += 1
            first_seen.setdefault(s[:2], e["seq"])
        cmd_list.append((e, read, written))

    out = ["## Frame layout (render targets, depth, motion vectors, resolution)", ""]
    if not cmd_list:
        return "\n".join(out + ["no commands", ""])
    out += ["Render targets by size and format (draws that wrote them):", "",
            table([(f"{w}x{h}", f, n) for (w, h, f), n in targets.most_common(20)], ["size", "format", "draws"]), "",
            "Depth buffers:", "",
            table([(f"{w}x{h}", f"{f} {DEPTH_FORMATS.get(f, '')}", n) for (w, h, f), n in depths.most_common(10)],
                  ["size", "format", "draws"]), "",
            "Textures written by compute (UAV), largest counts:", "",
            table([(f"{w}x{h}", f, n) for (w, h, f), n in uavs.most_common(15)], ["size", "format", "dispatches"]), ""]

    # internal resolution: the size of the depth buffer written by the most draws (main geometry)
    big_depth = [s for s in depths if s[0] * s[1] >= 640 * 360]
    internal = max(big_depth, key=lambda s: depths[s])[:2] if big_depth else None
    # output: the largest size written at all, preferring the last-written 4-channel 8/10-bit target
    sizes = [s for s in writes_by_size if s[0] * s[1] >= 640 * 360]
    output = max(sizes, key=lambda s: (s[0] * s[1], writes_by_size[s])) if sizes else None
    out.append(f"Most likely internal render resolution (size of the depth buffer most draws write): "
               f"**{internal[0]}x{internal[1]}**" if internal else "No large depth buffer found.")
    out.append("")
    if output:
        out.append(f"Largest size written in the frame (output / presentation): **{output[0]}x{output[1]}**")
        if internal and output != internal:
            out.append(f"(ratio {output[0] / internal[0]:.3f} x {output[1] / internal[1]:.3f}: "
                       "the frame is drawn smaller and scaled up)")
        out.append("")

    motion = [(s, n) for s, n in list(targets.items()) + list(uavs.items())
              if s[2] in MOTION_FORMATS and (internal is None or s[:2] == internal)]
    if motion:
        out += ["Motion-vector candidates (two-channel 16-bit at the internal size):", "",
                table([(f"{w}x{h}", f"{f} {MOTION_FORMATS[f]}", n) for (w, h, f), n in motion],
                      ["size", "format", "writes"]), ""]

    if internal and output and output != internal:
        cands = []
        for e, read, written in cmd_list:
            reads_internal = [r for r in read if r[:2] == internal]
            writes_bigger = [w for w in written if w[0] * w[1] > internal[0] * internal[1] * 1.2]
            if reads_internal and writes_bigger:
                score = len(reads_internal) + 2 * any(r[2] in MOTION_FORMATS for r in reads_internal) \
                    + 2 * any(r[2] in DEPTH_FORMATS or r[2] in (41, 46) for r in reads_internal)
                cands.append((score, e, reads_internal, writes_bigger))
        cands.sort(key=lambda c: (-c[0], c[1]["seq"]))
        if cands:
            out += ["Upscale pass candidates (read textures at the internal size, write at a larger "
                    "size; higher score = also reads depth / motion-like textures):", "",
                    table([(c[1]["seq"], c[0], name_of(c[1]["text"]),
                            ", ".join(f"{w}x{h} f{f}" for w, h, f in c[2][:6]),
                            ", ".join(f"{w}x{h} f{f}" for w, h, f in c[3][:3]))
                           for c in cands[:12]], ["line", "score", "pipeline", "reads (internal size)", "writes"]), ""]
        else:
            out += ["No command reads at the internal size and writes larger.", ""]
    return "\n".join(out)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("folder")
    ap.add_argument("--top", type=int, default=25)
    ap.add_argument("--out")
    a = ap.parse_args()
    text = report(a.folder, a.top)
    if a.out:
        with open(a.out, "w", encoding="utf-8") as f:
            f.write(text)
        print(f"written {a.out} ({len(text)} bytes)")
    else:
        sys.stdout.write(text)


if __name__ == "__main__":
    main()
