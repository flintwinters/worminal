"""Execute the real embedded installer against a private prefix; invoked only by the SSH fixture."""
import json
import os
import shlex
import sys

if trace := os.environ.get('WORMINAL_TEST_SSH_TRACE'):
    from pathlib import Path
    with Path(trace).open('a') as log:
        log.write('authenticated\n')

if pid_file := os.environ.get('WORMINAL_TEST_SSH_PID'):
    from pathlib import Path
    Path(pid_file).write_text(str(os.getpid()))

scenario = os.environ.get('WORMINAL_TEST_INSTALL_SCENARIO', 'success')
if scenario == 'missing-python':
    raise SystemExit(127)
if scenario in ('os', 'cpu'):
    print(json.dumps({'ready': True, 'system': 'Darwin' if scenario == 'os' else 'Linux',
                      'machine': 'wrong-cpu'}), flush=True)
    sys.stdin.buffer.read(1)
    raise SystemExit(0)
arguments = ['python3'] + sys.argv[1:3] if sys.argv[1:2] == ['-c'] else shlex.split(' '.join(sys.argv[-3:]))
assert arguments[:2] == ['python3', '-c']
namespace = {}
prefix, suffix = arguments[2].rsplit('exec(SOURCE)', 1)
exec(prefix, namespace)
source = namespace['SOURCE']
source = source.replace("PREFIX = Path('/usr/local')",
                        'PREFIX = Path('+repr(os.environ['WORMINAL_TEST_INSTALL_PREFIX'])+')')
source = source.replace("shutil.which('worminald') or shutil.which('worminal')", 'False')
source = source.replace('def legacy_workspace():', 'def legacy_workspace():\n    return False')
source = source.replace('def active_workspace():', 'def active_workspace():\n    return '+str(scenario == 'active'))
# The fixture target's libraries are not probed: the scenario says whether it can run graphics.
source = source.replace('def graphics_available():',
                        'def graphics_available():\n    return ' + str(os.environ.get('WORMINAL_TEST_GRAPHICS') == '1'))
if scenario == 'runtime':
    # Deterministic runtime rejection after receiving and staging the real binary.
    needle = "if result.returncode or b'Worp' not in result.stdout:"
    assert needle in source
    source = source.replace(needle, 'if True:')
if gate := os.environ.get('WORMINAL_TEST_UPDATE_GATE'):
    source = source.replace('def message(value):',
        'def message(value):\n'
        '    if value.get("installed") or value.get("current"):\n'
        '        import time\n'
        f'        gate = Path({gate!r})\n'
        '        gate.with_suffix(".waiting").touch()\n'
        '        while not gate.exists(): time.sleep(.01)')
if reply := os.environ.get('WORMINAL_TEST_MASTER_REPLY'):
    bridge = "os.execv(str(PREFIX/'bin/worminald'), ['worminald', bridge])"
    assert bridge in suffix
    suffix = suffix.replace(bridge, f'os.write(1, bytes.fromhex({reply!r}))')
code = f'LAYOUT={namespace["LAYOUT"]!r}\nMODE={namespace.get("MODE", "install")!r}\nSOURCE={source!r}\nexec(SOURCE)\n{suffix}'
os.execv('/usr/bin/python3', ['/usr/bin/python3', '-c', code])
