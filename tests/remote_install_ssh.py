"""Execute the real embedded installer against a private prefix; invoked only by the SSH fixture."""
import json
import os
import shlex
import sys

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
exec(arguments[2].rsplit('exec(SOURCE)', 1)[0], namespace)
source = namespace['SOURCE']
# This bootstrap models a publisher owning its private fixture prefix. NFS may expose
# another storage UID; changing only the test identity keeps actual owner/mode checks intact.
prefix = "PREFIX = Path('/usr/local')"
assert source.count(prefix) == 1, 'fixture must replace the live installation prefix'
source = source.replace(prefix,
                        'PREFIX = Path('+repr(os.environ['WORMINAL_TEST_INSTALL_PREFIX'])+')\n'
                        'os.geteuid = lambda: PREFIX.stat().st_uid')
source = source.replace("shutil.which('worminald') or shutil.which('worminal')", 'False')
source = source.replace('def legacy_workspace():', 'def legacy_workspace():\n    return False')
source = source.replace('def active_workspace():', 'def active_workspace():\n    return '+str(scenario == 'active'))
if scenario == 'runtime':
    # Deterministic runtime rejection after receiving and staging the real binary.
    needle = "if result.returncode or b'Worp' not in result.stdout:"
    assert needle in source
    source = source.replace(needle, 'if True:')
code = f'LAYOUT={namespace["LAYOUT"]!r}\nMODE={namespace.get("MODE", "install")!r}\nSOURCE={source!r}\nexec(SOURCE)\n'
os.execv('/usr/bin/python3', ['/usr/bin/python3', '-c', code])
