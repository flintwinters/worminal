"""Run the real embedded SSH agent against a private prefix, replacing only its fixed destination."""
import ast
import os
from pathlib import Path
import shlex
import sys

assert sys.argv[1:5] == ['-T', '-oBatchMode=yes', '-oConnectTimeout=10', 'fixture-host']
source = shlex.split(sys.argv[-1])[0]
values = {node.targets[0].id: ast.literal_eval(node.value) for node in ast.parse(source).body
          if isinstance(node, ast.Assign)}
agent = values['AGENT_SOURCE'].replace("PREFIX = Path('/usr/local')",
                                       'PREFIX = Path(' + repr(os.environ['UPGRADE_TARGET_PREFIX']) + ')')
code = (f'LAYOUT={values["LAYOUT"]!r}\nSUPPORT_SOURCE={values["SUPPORT_SOURCE"]!r}\n'
        f'AGENT_SOURCE={agent!r}\nexec(AGENT_SOURCE)')
os.execv(sys.executable, [sys.executable, '-c', code])
