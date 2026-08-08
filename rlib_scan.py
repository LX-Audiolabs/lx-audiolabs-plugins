import struct, sys, os

def scan_coff(data, name, problems):
    if len(data) < 20:
        return
    machine, nsec = struct.unpack_from('<HH', data, 0)
    if machine != 0x8664:
        return
    opt = struct.unpack_from('<H', data, 16)[0]
    shoff = 20 + opt
    for i in range(nsec):
        so = shoff + 40 * i
        if so + 40 > len(data):
            problems.append(f'{name}: truncated section header {i}')
            return
        prel = struct.unpack_from('<I', data, so + 24)[0]
        nrel = struct.unpack_from('<H', data, so + 32)[0]
        if prel == 0 or nrel == 0:
            continue
        off = prel
        if nrel == 0xFFFF:
            if off + 10 > len(data):
                problems.append(f'{name}: truncated reloc overflow')
                continue
            nrel = struct.unpack_from('<I', data, off)[0]
            # count includes the first entry; entries still start at off
        if nrel > 10_000_000:
            problems.append(f'{name} sec{i}: absurd nrel {nrel}')
            continue
        for j in range(nrel):
            ro = off + 10 * j
            if ro + 10 > len(data):
                problems.append(f'{name} sec{i}: truncated reloc {j}')
                break
            rtype = struct.unpack_from('<H', data, ro + 8)[0]
            if rtype > 0x0026:
                ctx = []
                for k in range(max(0, j - 3), min(nrel, j + 4)):
                    ko = off + 10 * k
                    if ko + 10 <= len(data):
                        ctx.append(struct.unpack_from('<H', data, ko + 8)[0])
                ctxs = ' '.join(f'0x{t:04X}' for t in ctx)
                problems.append(f'{name} sec{i} reloc{j}: bad type 0x{rtype:04X} neighbors: {ctxs}')
                break  # one per section is enough

def scan_rlib(path):
    problems = []
    with open(path, 'rb') as f:
        data = f.read()
    if not data.startswith(b'!<arch>\n'):
        return problems
    off = 8
    long_names = b''
    while off + 60 <= len(data):
        hdr = data[off:off + 60]
        if hdr[58:60] != b'`\n':
            break
        mname = hdr[0:16].decode('ascii', 'replace').strip()
        try:
            size = int(hdr[48:58].decode('ascii').strip())
        except ValueError:
            break
        body_off = off + 60
        body = data[body_off:body_off + size]
        if mname == '//':
            long_names = body
        else:
            real = mname
            if mname.startswith('/') and mname[1:].isdigit():
                no = int(mname[1:])
                for sep in (b'\x00', b'\n'):
                    end = long_names.find(sep, no)
                    if end != -1:
                        real = long_names[no:end].decode('ascii', 'replace').rstrip('/').strip()
                        break
            if real.endswith('.o') or real.endswith('.obj'):
                scan_coff(body, real, problems)
        off = body_off + size + (size & 1)
    return problems

bad = []
for d in sys.argv[1:]:
    for fn in sorted(os.listdir(d)):
        if fn.endswith('.rlib'):
            p = os.path.join(d, fn)
            probs = scan_rlib(p)
            if probs:
                bad.append(p)
                print(f'CORRUPT: {p}')
                for pr in probs[:3]:
                    print(f'    {pr}')
print(f'---\n{len(bad)} corrupt rlib(s)')
