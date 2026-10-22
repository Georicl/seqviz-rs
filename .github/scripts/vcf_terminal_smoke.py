#!/usr/bin/env python3
"""Exercise VCF screen geometry, colours and input through a real PTY and pyte.

Build target/release/sq first, or set SQ_BIN to a different binary. OSC 52 frames
are compared byte-for-byte without using the system clipboard. Fixtures, screen
snapshots and ANSI transcripts stay in a temporary directory.
"""
import base64
import codecs
import errno
import fcntl
import gzip
import json
import os
from pathlib import Path
import re
import select
import signal
import struct
import subprocess
import tempfile
import termios
import time
import unicodedata

import pyte


ROOT = Path(__file__).resolve().parents[2]
BINARY = Path(os.environ.get("SQ_BIN", ROOT / "target/release/sq")).resolve()
OSC52 = re.compile(rb"\x1b\]52;c;([A-Za-z0-9+/=]*)(?:\x07|\x1b\\)")
OUTPUT = Path(tempfile.mkdtemp(prefix="seqviz-vcf-terminal-"))
RESULTS = []


class Session:
    def __init__(self, name, paths, width=120, height=32):
        self.name = name
        self.directory = OUTPUT / name
        self.directory.mkdir()
        self.master, self.slave = os.openpty()
        self.original = termios.tcgetattr(self.slave)
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
        self.screen = pyte.Screen(width, height)
        self.stream = pyte.Stream(self.screen)
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")
        self.raw = bytearray()
        self.process = subprocess.Popen(
            [str(BINARY), *map(str, paths)], stdin=self.slave, stdout=self.slave,
            stderr=self.slave, start_new_session=True,
            env={**{key: value for key, value in os.environ.items() if key != "NO_COLOR"},
                 "TERM": "xterm-256color", "COLORTERM": "truecolor", "CLICOLOR_FORCE": "1",
                 "SSH_CONNECTION": "seqviz-vcf-pty-check",
                 "XDG_CACHE_HOME": str(self.directory / "cache"),
                 "XDG_CONFIG_HOME": str(self.directory / "config")},
        )

    def read(self, timeout=0.05):
        if select.select([self.master], [], [], timeout)[0]:
            try:
                data = os.read(self.master, 65536)
                self.raw.extend(data)
                self.stream.feed(self.decoder.decode(data))
            except OSError as error:
                if error.errno != errno.EIO:
                    raise

    def settle(self, duration=0.2):
        until = time.monotonic() + duration
        while time.monotonic() < until:
            self.read(min(0.05, max(0, until - time.monotonic())))

    def text(self):
        return "\n".join(self.screen.display)

    def wait(self, test, label, timeout=8):
        until = time.monotonic() + timeout
        while time.monotonic() < until:
            self.read()
            if test():
                self.settle(0.1)
                return
            if self.process.poll() is not None:
                break
        self.snapshot(label + "-timeout")
        raise AssertionError(label + "\n" + self.text())

    def send(self, keys):
        os.write(self.master, keys)
        self.settle()

    def mouse(self, button, x, y, release=False):
        suffix = "m" if release else "M"
        self.send(f"\x1b[<{button};{x + 1};{y + 1}{suffix}".encode())

    def click(self, x, y):
        self.mouse(0, x, y)
        self.mouse(0, x, y, release=True)

    def resize(self, width, height):
        self.screen.resize(lines=height, columns=width)
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
        self.process.send_signal(signal.SIGWINCH)
        self.settle()

    def row(self, text):
        for y, line in enumerate(self.screen.display):
            if text in line:
                return y, line.index(text)
        raise AssertionError(f"Missing screen text: {text}\n{self.text()}")

    def snapshot(self, label):
        (self.directory / (label + ".txt")).write_text(self.text())
        cells = {str(y): [{"text": self.screen.buffer[y][x].data,
                           "fg": self.screen.buffer[y][x].fg,
                           "bg": self.screen.buffer[y][x].bg,
                           "bold": self.screen.buffer[y][x].bold}
                          for x in range(self.screen.columns)]
                 for y in range(self.screen.lines)}
        (self.directory / (label + ".json")).write_text(json.dumps(cells, ensure_ascii=False))
        (self.directory / "terminal.ansi").write_bytes(self.raw)

    def check(self, condition, label, detail=""):
        RESULTS.append({"case": self.name, "check": label, "passed": bool(condition), "detail": detail})
        print(f"{'PASS' if condition else 'FAIL'} {self.name}/{label}" + (f": {detail}" if not condition else ""), flush=True)
        self.snapshot(label)

    def copy_payload(self, label, key=b"y"):
        before = len(OSC52.findall(self.raw))
        self.send(key)
        self.wait(lambda: len(OSC52.findall(self.raw)) > before, label + "-copy")
        return base64.b64decode(OSC52.findall(self.raw)[before], validate=True)

    def copy(self, expected, label, key=b"y"):
        value = self.copy_payload(label, key)
        self.check(value == expected, label, repr(value) if value != expected else "")
        return value

    def close(self):
        if self.process.poll() is None:
            self.send(b"\x03")
            until = time.monotonic() + 5
            while self.process.poll() is None and time.monotonic() < until:
                self.read()
            self.process.wait(timeout=1)
        self.settle(0.05)
        flags = termios.ICANON | termios.ECHO
        self.check(self.process.returncode == 0
                   and termios.tcgetattr(self.slave)[3] & flags == self.original[3] & flags
                   and b"\x1b[?1049l" in self.raw, "terminal-restored")
        os.close(self.master)
        os.close(self.slave)


def fixture():
    directory = OUTPUT / "fixtures"
    directory.mkdir()
    samples = ["SAMPLE_WITH_A_VERY_LONG_NAME_000", *[f"SAMPLE_{i:03}" for i in range(1, 18)]]
    long_value = "fragment_" * 40 + "END_INFO"
    header = "\n".join([
        "##fileformat=VCFv4.3",
        '##INFO=<ID=DP,Number=1,Type=Integer,Description="Depth">',
        '##INFO=<ID=LONG,Number=1,Type=String,Description="Long field">',
        '##INFO=<ID=UNICODE,Number=1,Type=String,Description="Unicode text">',
        '##FILTER=<ID=q10,Description="Quality filter">',
        '##FORMAT=<ID=GT,Number=1,Type=String,Description="Genotype">',
        '##FORMAT=<ID=GQ,Number=1,Type=Integer,Description="Genotype quality">',
        '##FORMAT=<ID=DP,Number=1,Type=Integer,Description="Depth">',
        '##FORMAT=<ID=AD,Number=R,Type=Integer,Description="Allelic depths">',
        *[f"##contig=<ID={name},length=100000>" for name in ["chr10", "chr1", "chr2", *[f"ctg{i:02}" for i in range(20)]]],
        "#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\tFORMAT\t" + "\t".join(samples),
    ]) + "\n"
    values = ["0|10/2/.:60:10:0,4,0,0,0,0,0,0,0,0,6"] + [f"1/2:{30+i}:10:0,4,6,0,0,0,0,0,0,0,0" for i in range(1, 18)]
    records = []
    for i in range(8):
        records.append("\t".join([
            "chr1", str(100 * (i+1)), f"rs{i+1:03}", "A", "C,G,T,AA,AC,AG,AT,CA,CG,CT",
            "75", ["PASS", "q10", "."][i % 3], f"DP={10+i};LONG={long_value};UNICODE=café界",
            "GT:GQ:DP:AD", *values,
        ]))
    for chrom in ["chr10", "chr2"]:
        records.append("\t".join([chrom, "100", f"{chrom}_id", "A", "C,G,T,AA,AC,AG,AT,CA,CG,CT", "20", "PASS", "DP=10", "GT:GQ:DP:AD", *values]))
    vcf = directory / "review.vcf"
    document = (header + "\n".join(records) + "\n").encode()
    vcf.write_bytes(document)
    compressed = directory / "review.vcf.gz"
    compressed.write_bytes(gzip.compress(document, mtime=0))
    fasta = directory / "mixed.fa"
    fasta.write_text(">qa_fasta_record\nACGTACGT\n")
    empty = directory / "header-only.vcf"
    empty.write_text(header)
    zero = directory / "empty.vcf"
    zero.write_bytes(b"")
    bad = directory / "bad.vcf"
    bad.write_text(header + records[0].replace("\t100\t", "\tBAD\t", 1) + "\n")
    return vcf, compressed, fasta, empty, zero, bad, header, records, values, long_value


def geometry(paths, records):
    for width, height in [(120, 32), (80, 24), (60, 20), (40, 12), (80, 13), (30, 12)]:
        session = Session(f"size-{width}x{height}", paths, width, height)
        try:
            session.wait(lambda: "VCFv4.3" in session.text() and "Variants" in session.text(), "ready")
            session.check("Variants" in session.text() and session.screen.display[-1].startswith("? help"), "table-and-footer")
            session.check(any(all(label in line for label in ["CHROM", "POS", "REF", "ALT"])
                              for line in session.screen.display), "essential-variant-columns")
            session.check(not re.search(r"\b1:\s*1:", session.screen.display[0]), "single-tab-number")
            session.copy(records[0].encode(), "table-copy-full-record")
            session.send(b"\r2")
            if "SAMPLE_WITH" in session.text():
                session.check("SAMPLE_WITH" in session.text() and "Details" in session.text(), "visible-sample-details")
            else:
                session.copy(records[0].encode(), "hidden-details-keep-table-focus")
        finally:
            session.close()


def interactions(vcf, compressed, fasta, header, records, values, long_value):
    session = Session("interaction", [vcf, fasta, compressed])
    try:
        session.wait(lambda: "chr1:100" in session.text() and "rs001" in session.text(), "ready")
        heading_y, qual_x = session.row("QUAL")
        row_y, _ = session.row("rs001")
        filter_x = session.screen.display[heading_y].index("FILTER")
        session.check(session.screen.buffer[row_y][qual_x].fg == "e0e5ef", "qual-normal-foreground")
        session.check(session.screen.buffer[row_y][filter_x].fg == "7ed397"
                      and session.screen.buffer[row_y+1][filter_x].fg == "f0c674"
                      and session.screen.buffer[row_y+2][filter_x].fg == "8f9bb3", "filter-pass-failed-missing-colours")
        tabs_y, sample_x = session.row("SAMPLES")
        session.click(sample_x+2, tabs_y)
        session.wait(lambda: "SAMPLE_WITH" in session.text(), "mouse-sample-tab")
        y, x = session.row("0|10/2/.")
        headings = [line for line in session.screen.display[:y] if "SAMPLE" in line and "GT" in line and "GQ" in line]
        session.check(bool(headings) and headings[-1].index("GT") == x, "long-sample-heading-alignment")
        expected = ["e0e5ef", "e0e5ef", "f18594", "f18594", "e0e5ef", "be9bf3", "e0e5ef", "8f9bb3"]
        session.check([session.screen.buffer[y][x+i].fg for i in range(8)] == expected, "gt-complete-tokens-and-missing-colours")
        bars = [session.screen.buffer[y][i].fg for i, ch in enumerate(session.screen.display[y]) if ch == "█"]
        session.check(bool(bars) and set(bars) == {"7ab7ff", "f18594"}, "ad-colours-follow-nonzero-alleles")
        sample_copy = ("SAMPLE\tGT\tGQ\tDP\tAD\nSAMPLE_WITH_A_VERY_LONG_NAME_000\t" + values[0].replace(":", "\t")).encode()
        session.copy(sample_copy, "mouse-sample-tab-copy-context")
        sample_y, sample_x = session.row("SAMPLE_WITH")
        session.click(sample_x, sample_y)
        session.copy(sample_copy, "single-click-sample-keeps-row-copy")
        session.mouse(0, sample_x, sample_y)
        session.mouse(32, sample_x+5, sample_y)
        session.mouse(0, sample_x+5, sample_y, release=True)
        session.copy(b"SAMPLE", "mouse-select-sample-text")
        session.click(6, 0)
        session.copy(b"SAMPLE", "current-tab-keeps-selection")

        session.send(b"2\x1b[F")
        session.wait(lambda: "SAMPLE_017" in session.text(), "last-sample-visible")
        session.copy(("SAMPLE\tGT\tGQ\tDP\tAD\nSAMPLE_017\t"+values[17].replace(":", "\t")).encode(), "keyboard-last-sample")
        session.resize(80, 16)
        session.check("SAMPLE_017" in session.text(), "resized-selected-sample-visible")
        session.copy(("SAMPLE\tGT\tGQ\tDP\tAD\nSAMPLE_017\t"+values[17].replace(":", "\t")).encode(), "resized-selected-sample-copy")
        session.resize(120, 32)
        detail_y, detail_x = session.row("SAMPLE_017")
        session.mouse(64, detail_x+1, detail_y)
        session.copy(("SAMPLE\tGT\tGQ\tDP\tAD\nSAMPLE_014\t"+values[14].replace(":", "\t")).encode(), "wheel-detail-scroll")
        session.send(b"1j" + b"l" * 90)
        session.check("END_INFO" in session.text(), "horizontal-long-field-visible")
        session.copy(("LONG="+long_value).encode(), "long-field-copy-untruncated")
        session.send(b"1jj")
        session.check("café界" in unicodedata.normalize("NFC", session.text()), "unicode-combining-display")
        session.copy("UNICODE=café界".encode(), "unicode-copy-original-text")

        contig_y, _ = session.row("Contig:")
        session.click(4, contig_y)
        session.wait(lambda: "Contig · type to search" in session.text(), "mouse-contig-picker")
        cy, cx = session.row("chr2  (")
        session.click(cx+2, cy)
        session.wait(lambda: "Contig: chr2" in session.text(), "mouse-contig-selected")
        session.send(b"\x1b")
        session.copy(records[9].encode(), "mouse-contig-record")
        session.send(b"cchr1\r:200\r")
        session.wait(lambda: "chr1:200" in session.text(), "keyboard-position")
        # The header lists chr10 before chr1; an exact picker match must win.
        session.copy(records[1].encode(), "keyboard-contig-position-record")
        session.send(b"m")
        session.wait(lambda: "Original VCF header" in session.text(), "header-modal")
        session.send(b"\x1b[F")
        session.check(bool(re.search(r"#CHROM\s+POS\s+ID\s+REF\s+ALT", session.text())), "header-columns-visibly-separated")
        session.copy(header.encode(), "original-header-preserves-tabs-and-newlines")
        session.send(b"\t")
        session.click(26, 0)
        session.check("Original VCF header" in session.text() and "qa_fasta_record" not in session.text(), "modal-captures-tab-key-and-mouse")
        session.send(b"q")
        session.click(26, 0)
        session.wait(lambda: "qa_fasta_record" in session.text(), "mouse-switch-to-fasta")
        session.copy(b">qa_fasta_record\nACGTACGT\n", "mixed-fasta-context")
        session.send(b"\x1b[Z")
        session.wait(lambda: "chr1:200" in session.text(), "back-to-vcf")
        session.copy(records[1].encode(), "vcf-state-after-mixed-tab")

        with_header = (header + records[1] + "\n").encode()
        session.copy(with_header, "table-header-and-record", key=b"Y")
        session.send(b"\r3")
        session.copy(records[1].encode(), "raw-detail-original-record")
        session.copy(with_header, "raw-detail-header-and-record", key=b"Y")
        session.resize(78, 24)
        session.copy(with_header, "resize-keeps-complete-header-and-record", key=b"Y")
        before = len(OSC52.findall(session.raw))
        session.send(b"cchr10\x1b")
        session.check(len(OSC52.findall(session.raw)) == before, "cancel-picker-emits-no-copy")
        session.copy(records[1].encode(), "cancel-picker-keeps-current-record")
        session.resize(120, 32)

        session.send(b"\x1b[Z")
        session.wait(lambda: "chr1:100" in session.text() and "Contig: All records" in session.text(), "gzip-first-render")
        session.copy(records[0].encode(), "gzip-original-record")
        session.send(b"\x1b[12~")
        information = session.copy_payload("gzip-file-information")
        session.check(b"Compression: gzip" in information and b"Format: VCFv4.3" in information,
                      "gzip-file-information", repr(information))
        session.send(b"qcchr1\r:200\r")
        session.wait(lambda: "chr1:200" in session.text(), "gzip-position")
        session.copy(with_header, "gzip-header-and-record", key=b"Y")
        session.send(b"\t")
        session.copy(records[1].encode(), "plain-tab-retains-raw-detail-context")
        session.send(b"m")
        session.resize(20, 6)
        session.send(b"q")
        session.check(session.process.poll() == 0, "small-terminal-q-exits-modal")
    finally:
        session.close()


def empty_and_errors(paths):
    for name, path, expected in [
        ("header-only", paths[0], "No records"),
        ("zero-byte", paths[1], "missing #CHROM"),
        ("invalid-record", paths[2], "POS"),
    ]:
        session = Session(name, [path], 80, 24)
        try:
            session.wait(lambda: expected in session.text(), "expected-state")
            session.check(True, "visible-empty-or-error-state")
            if name == "header-only":
                session.check("No record selected" in session.text(), "empty-detail-not-loading")
        finally:
            session.close()


def main():
    if not BINARY.is_file() or not os.access(BINARY, os.X_OK):
        raise SystemExit(f"Build sq first, or set SQ_BIN: {BINARY}")
    vcf, compressed, fasta, empty, zero, bad, header, records, values, long_value = fixture()
    for name, action in [
        ("geometry", lambda: geometry([vcf, fasta], records)),
        ("interactions", lambda: interactions(vcf, compressed, fasta, header, records, values, long_value)),
        ("empty-and-errors", lambda: empty_and_errors([empty, zero, bad])),
    ]:
        try:
            action()
        except Exception as error:
            RESULTS.append({"case": name, "check": "completed", "passed": False, "detail": str(error)})
            print(f"FAIL {name}: {error}", flush=True)
    (OUTPUT / "results.json").write_text(json.dumps(RESULTS, ensure_ascii=False, indent=2))
    failures = [result for result in RESULTS if not result["passed"]]
    print(f"Artifacts: {OUTPUT}\nChecks: {len(RESULTS)}; failures: {len(failures)}", flush=True)
    raise SystemExit(bool(failures))


if __name__ == "__main__":
    main()
