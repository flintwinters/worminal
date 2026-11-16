"""Check real responsive layouts in private headless Chromium; preserve captures and profiles."""
import base64
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import shutil
import subprocess
import sys
import time
import unittest
from urllib.request import urlopen

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from http_fixture import serving
from tools import site
from tools.paths import CHECKS


class DevTools:
    def __init__(self, address):
        import websocket
        self.socket = websocket.create_connection(address, timeout=40, suppress_origin=True)
        self.serial = 0

    def call(self, method, **params):
        self.serial += 1
        self.socket.send(json.dumps({'id': self.serial, 'method': method, 'params': params}))
        while True:
            response = json.loads(self.socket.recv())
            if response.get('id') == self.serial:
                if 'error' in response:
                    raise AssertionError(response['error'])
                return response.get('result', {})

    def value(self, expression):
        result = self.call('Runtime.evaluate', expression=expression, returnByValue=True)
        if 'exceptionDetails' in result:
            raise AssertionError(result['exceptionDetails'])
        return result['result'].get('value')


class SitePreview(unittest.TestCase):
    def test_desktop_and_mobile(self):
        browser = shutil.which('chromium')
        self.assertIsNotNone(browser, 'Install Chromium and websocket-client for optional browser QA')
        output = site.build()

        class Handler(SimpleHTTPRequestHandler):
            def translate_path(self, path):
                if path.startswith('/worminal/'):
                    path = path.removeprefix('/worminal')
                return super().translate_path(path)

            def log_message(self, *_args):
                pass

        server = ThreadingHTTPServer(('127.0.0.1', 0), partial(Handler, directory=str(output)))
        diagnostics = CHECKS / 'site-preview'
        diagnostics.mkdir(parents=True, exist_ok=True)
        with serving(server):
            for label, width, height in (('desktop', 1440, 1100), ('mobile', 390, 1000)):
                profile = diagnostics / (label + '-profile')
                marker = profile / 'DevToolsActivePort'
                marker.unlink(missing_ok=True)
                with (diagnostics / f'{label}.log').open('wb') as log:
                    process = subprocess.Popen([
                        browser, '--headless', '--no-sandbox', '--disable-gpu',
                        '--disable-background-networking', '--no-first-run', '--remote-debugging-port=0',
                        f'--user-data-dir={profile}', 'about:blank'], stdout=log, stderr=log)
                    devtools = None
                    try:
                        deadline = time.monotonic() + 60
                        while not marker.exists():
                            self.assertIsNone(process.poll(), f'Chromium failed; see {label}.log')
                            self.assertLess(time.monotonic(), deadline, 'Chromium debugger did not start')
                            time.sleep(.05)
                        port = marker.read_text().splitlines()[0]
                        with urlopen(f'http://127.0.0.1:{port}/json/list', timeout=5) as response:
                            pages = json.load(response)
                        address = next(page['webSocketDebuggerUrl'] for page in pages if page['type'] == 'page')
                        devtools = DevTools(address)
                        # CLI --window-size has a minimum native width: emulate the actual phone viewport.
                        devtools.call('Emulation.setDeviceMetricsOverride', width=width, height=height,
                                      deviceScaleFactor=1, mobile=label == 'mobile')
                        url = f'http://127.0.0.1:{server.server_port}/worminal/'
                        devtools.call('Page.navigate', url=url)
                        while not devtools.value(f'location.href === {json.dumps(url)} && '
                                                'document.readyState === "complete"'):
                            self.assertLess(time.monotonic(), deadline, 'Site did not finish loading')
                            time.sleep(.05)
                        self.assertEqual(devtools.value('innerWidth'), width)
                        self.assertLessEqual(devtools.value('document.documentElement.scrollWidth'), width,
                                             f'{label} layout overflows horizontally')
                        self.assertGreater(devtools.value('document.documentElement.scrollHeight'), height,
                                           'The landing page must scroll naturally')
                        self.assertTrue(devtools.value('[...document.querySelectorAll("[data-panel]")].every('
                            'panel => !panel.hidden && getComputedStyle(panel).display !== "none")'))
                        self.assertEqual(devtools.value(
                            'getComputedStyle(document.querySelector(".table-scroll")).overflowY'), 'visible')
                        devtools.call('Page.bringToFront')
                        devtools.value('document.querySelector("a.tab[href=\\"#panel-agents\\"]").focus()')
                        for event in ('keyDown', 'keyUp'):
                            devtools.call('Input.dispatchKeyEvent', type=event, key='Enter', code='Enter',
                                          windowsVirtualKeyCode=13)
                        self.assertEqual(devtools.value('location.hash'), '#panel-agents')
                        self.assertGreater(devtools.value('scrollY'), 0)
                        devtools.value('document.getElementById("find").value = "guarded"; '
                                       'document.getElementById("find").dispatchEvent(new Event("input"))')
                        self.assertEqual(devtools.value(
                            'document.querySelectorAll("#capabilities tbody tr:not([hidden])").length'), 1)
                        devtools.value('document.getElementById("find").value = ""; '
                                       'document.getElementById("find").dispatchEvent(new Event("input")); '
                                       'document.querySelector("a.tab[href=\\"#panel-sessions\\"]").click()')
                        devtools.call('Browser.grantPermissions',
                                      origin=f'http://127.0.0.1:{server.server_port}',
                                      permissions=['clipboardReadWrite', 'clipboardSanitizedWrite'])
                        devtools.call('Page.bringToFront')
                        devtools.call('Runtime.evaluate', userGesture=True,
                                      expression='document.querySelector("[data-copy=command-sessions]").click()')
                        deadline = time.monotonic() + 3
                        while not devtools.value(
                                'document.getElementById("status-message").textContent.startsWith("Copied:")'):
                            self.assertLess(time.monotonic(), deadline, 'Copy command did not complete')
                            time.sleep(.02)
                        copied = devtools.call('Runtime.evaluate', expression='navigator.clipboard.readText()',
                                               userGesture=True, awaitPromise=True, returnByValue=True)
                        self.assertEqual(copied['result']['value'], 'worminal --attach')
                        devtools.value('scrollTo(0, 0)')
                        result = devtools.call('Page.captureScreenshot', format='png', captureBeyondViewport=False)
                        (diagnostics / f'{label}.png').write_bytes(base64.b64decode(result['data']))
                        if label == 'desktop':
                            devtools.call('Emulation.setDeviceMetricsOverride', width=1050, height=850,
                                          deviceScaleFactor=1, mobile=False)
                            self.assertNotEqual(devtools.value(
                                'getComputedStyle(document.querySelector(".reference-pane")).display'), 'none')
                            self.assertNotEqual(devtools.value(
                                'getComputedStyle(document.querySelector(".index-pane")).display'), 'none')
                            self.assertLessEqual(devtools.value('document.documentElement.scrollWidth'), 1050)
                            self.assertGreater(devtools.value('document.documentElement.scrollHeight'), 850)
                            result = devtools.call('Page.captureScreenshot', format='png', captureBeyondViewport=False)
                            (diagnostics / 'middle.png').write_bytes(base64.b64decode(result['data']))
                        devtools.value('document.getElementById("get-worminal").scrollIntoView()')
                        self.assertGreater(devtools.value('scrollY'), 0)
                        self.assertLess(devtools.value(
                            'document.getElementById("get-worminal").getBoundingClientRect().top'),
                            devtools.value('innerHeight'))
                        result = devtools.call('Page.captureScreenshot', format='png', captureBeyondViewport=False)
                        (diagnostics / f'{label}-bottom.png').write_bytes(base64.b64decode(result['data']))
                    finally:
                        if devtools is not None:
                            devtools.socket.close()
                        process.terminate()
                        try:
                            process.wait(timeout=10)
                        except subprocess.TimeoutExpired:
                            process.kill()
                            process.wait(timeout=5)
        print(f'Private site previews: {diagnostics}')


if __name__ == '__main__':
    unittest.main()
