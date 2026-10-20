#!/usr/bin/env python3
"""Real PTY acceptance checks. Requires pyte; build with cargo build --release first."""
import base64, errno, fcntl, os, re, select, signal, struct, subprocess, sys, tempfile, termios, time
from pathlib import Path
import pyte

root = Path(__file__).resolve().parent.parent
binary = Path(os.environ.get('SQ_BIN', root / 'target/release/sq'))
out = Path(tempfile.gettempdir()) / 'seqviz-ui-acceptance'
out.mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(prefix='seqviz-ui-') as tmp:
    tmp = Path(tmp)
    fa = tmp / 'sequence.fa'
    fa.write_text('>seq1 description\n' + 'ACGT' * 2000 + '\n>seq2\nTTTTCCCC\n')
    fq = tmp / 'reads.fastq'
    fq.write_text('@INST:1:FLOW:2:3:4:5 1:N:0:ACGT\n' + 'ACGT' * 40 + '\n+\n' + '!5I~' * 40 + '\n')
    master, slave = os.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 28, 100, 0, 0))
    original = termios.tcgetattr(slave)
    env = {**os.environ, 'TERM':'xterm-256color', 'SSH_CONNECTION':'seqviz-pty-check', 'XDG_CACHE_HOME':str(tmp/'cache'), 'XDG_CONFIG_HOME':str(tmp/'config')}
    process = subprocess.Popen([str(binary), str(fa), str(fq), str(root/'test/alignments.sam'), str(root/'test/alignments.bam')], stdin=slave, stdout=slave, stderr=slave, env=env, start_new_session=True)
    screen = pyte.Screen(100, 28)
    stream = pyte.Stream(screen)
    raw = bytearray()
    def read(timeout=.05):
        ready, _, _ = select.select([master], [], [], timeout)
        if ready:
            try: data=os.read(master, 65536)
            except OSError as e:
                if e.errno == errno.EIO: return
                raise
            raw.extend(data)
            stream.feed(data.decode('utf-8','replace'))
    def content(): return '\n'.join(screen.display)
    def wait_for(test, label, timeout=8):
        until=time.monotonic()+timeout
        while time.monotonic()<until:
            read()
            if test(content()):
                (out/(label+'.txt')).write_text(content())
                print(label+': passed', flush=True)
                return
            if process.poll() is not None: break
        raise AssertionError(label+'\n'+content()+'\nRAW: '+repr(bytes(raw[-1000:])))
    def send(data): os.write(master,data)
    started=time.monotonic()
    try:
        wait_for(lambda s:'seq1 description' in s and 'ACGTACGT' in s, '01-fasta')
        print('PTY first rendered sequence %.1f ms' % ((time.monotonic()-started)*1000), flush=True)
        send(b']')
        wait_for(lambda s:'seq2' in s and 'TTTTCCCC' in s and 'record 2 /' in s, '02-next-record')
        send(b'/seq1\r')
        wait_for(lambda s:'seq1 description' in s and 'Match found' in s, '03-search')
        send(b':5\r')
        wait_for(lambda s:' 5 ' in s and 'seq1 description' in s, '04-position')
        send(b'vll')
        for _ in range(5): read()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 70, 0, 0))
        screen.resize(lines=24, columns=70)
        process.send_signal(signal.SIGWINCH)
        wait_for(lambda s:'seq1 description' in s and 'ACGTACGT' in s and '│' == screen.display[6][17] and re.search(r'\d+\s+[ACGT]{40,}│$', screen.display[20]) and screen.display[23].startswith('? help'), '05-resize')
        # Hold a drag at the lower edge without sending further mouse movement.
        # The selection must extend as the viewport scrolls.
        # The mouse press replaces the previous selection. A bare Escape here
        # can merge with the following mouse escape sequence in the PTY parser.
        rows = []
        for y, line in enumerate(screen.display):
            match = re.search(r'(\d+)\s+([ACGT]{6,})', line)
            if match: rows.append((y, match))
        assert len(rows) > 2, content()
        top_y, top = rows[0]
        bottom_y, bottom = rows[-1]
        x = top.start(2)
        initial_length = int(bottom.group(1)) - int(top.group(1)) + 1
        send(f'\x1b[<0;{x+1};{top_y+1}M'.encode())
        for _ in range(3): read()
        send(f'\x1b[<32;{x+1};{bottom_y+1}M'.encode())
        deadline = time.monotonic() + .7
        while time.monotonic() < deadline: read()
        send(f'\x1b[<0;{x+1};{bottom_y+1}m'.encode())
        send(b'y')
        wait_for(lambda s:'OSC 52' in s, '05b-edge-drag-copy')
        copies = re.findall(rb'\x1b\]52;c;([^\x07]*)\x07', bytes(raw))
        assert copies, 'No clipboard request was emitted'
        copied = base64.b64decode(copies[-1])
        assert len(copied) > initial_length, (len(copied), initial_length)
        assert not re.search(rb'[^ACGT]', copied), copied[:100]
        print('Edge drag expanded selection from %d to %d bases' % (initial_length, len(copied)), flush=True)
        send(b'?')
        wait_for(lambda s:'Help' in s and 'BROWSE' in s, '06-help')
        send(b'q\x1b[14~')
        wait_for(lambda s:'Statistics' in s and 'N50:' in s, '07-statistics')
        send(b'q\t')
        wait_for(lambda s:'FASTQ' in s and 'Quality:' not in s and 'Q ' in s and '!5I~' in s, '08-fastq')
        send(b'Q')
        wait_for(lambda s:'FASTQ' in s and '!5I~' not in s, '09-quality-toggle')
        send(b'\t')
        wait_for(lambda s:'SAM ·' in s and 'record 1 /' in s, '10-sam')
        send(b'y')
        wait_for(lambda s:'Copy is disabled for SAM/BAM' in s, '11-copy-gate')
        send(b'\x1b[13~')
        wait_for(lambda s:'Current record' in s and 'FLAG:' in s and 'CIGAR:' in s, '12-alignment-properties')
        send(b'q\t')
        wait_for(lambda s:'BAM · BGZF' in s, '13-bam')
        send(b'x')
        wait_for(lambda s:'seq1 description' in s, '14-close-tab')
        send(b'q')
        process.wait(timeout=5)
        while select.select([master],[],[],.05)[0]: read(0)
        assert process.returncode==0, process.returncode
        restored=termios.tcgetattr(slave)
        assert restored[3] & (termios.ICANON|termios.ECHO) == original[3] & (termios.ICANON|termios.ECHO)
        assert b'\x1b[?1049l' in raw
        print('15-quit-and-terminal-restoration: passed')
    finally:
        if process.poll() is None: process.kill(); process.wait()
        (out/'terminal.ansi').write_bytes(raw)
        os.close(master); os.close(slave)
