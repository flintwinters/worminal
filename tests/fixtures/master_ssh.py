"""Run the real authenticated view bootstrap against the current private test daemon."""
import shlex
import subprocess
import sys


def run(daemon):
    arguments = shlex.split(' '.join(sys.argv[5:]))
    assert arguments[:2] == ['python3', '-c'], arguments[:2]
    prefix, suffix = arguments[2].split('exec(SOURCE)', 1)
    namespace = {'__name__': '__main__'}
    exec(prefix, namespace)
    source = namespace['SOURCE']
    version = subprocess.check_output([daemon, '--version'], text=True).strip()
    source = source.replace('def installed_version(layout):',
                            f'def installed_version(layout):\n    return {version!r}')
    # Equal versions never publish. Only config exchange and bridge execute the private daemon.
    suffix = suffix.replace("str(PREFIX/'bin/worminald')", repr(str(daemon)))
    namespace['SOURCE'] = source
    exec('exec(SOURCE)' + suffix, namespace)
