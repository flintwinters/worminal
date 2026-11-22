"""Split panes in the actual Rust app on private displays: tab drags, keyboard moves, dividers, browser panes and
tab strips that rewrap with their pane's width."""
from contextlib import ExitStack
from pathlib import Path
from types import SimpleNamespace
import json
import os
import shlex
import shutil
import subprocess
import time
import unittest

from runtime import CHECKS, runtime_binary, window, stop_application
from visible_latency import image_bytes
from x11 import isolated_display
from preview import profile

ROOT = Path(__file__).resolve().parent.parent


def window_size(root):
    geometry = root.get_geometry()
    return geometry.width, geometry.height


def fix_size(root, xdo, wid, width, height):
    """Resize once the first terminal has fitted the window to its character grid; returns the settled size."""
    def settle():
        last, since = None, time.monotonic()
        while time.monotonic() - since < .5:
            if window_size(root) != last:
                last, since = window_size(root), time.monotonic()
            time.sleep(.02)
        return last
    settle()
    xdo('windowsize', wid, str(width), str(height))
    return settle()


class SplitPaneTest(unittest.TestCase):
    RED, BLUE = (200, 30, 30), (20, 40, 210)

    def colored_split_window(self, env, cleanup, then=None):
        """A 900x500 window whose two terminal tabs repaint red and blue (titled so), the blue one selected; probes
        for its pixels. With `then`, the first blue shell runs that command once `trigger()` is called."""
        from Xlib import X, display, error
        from PIL import Image
        red, blue = self.RED, self.BLUE
        # Each terminal repaints its whole screen continuously: resizing keeps text but not erased background.
        # Typing through the agent API would take the window's control of the tab, so no tab is typed into.
        fill = lambda color, title: ("while :; do printf '\\033]2;%s\\007\\033[48;2;%d;%d;%dm\\033[2J'; sleep .2; done"
                                     % ((title,) + color))
        script, trigger = CHECKS / 'split-shell.sh', CHECKS / 'split-shell.trigger'
        trigger.unlink(missing_ok=True)
        blue_shell = fill(blue, 'blue') if then is None else (
            fill(blue, 'blue').replace('while :', f'until [ -e {shlex.quote(str(trigger))} ]')
            + f'; rm {shlex.quote(str(trigger))}; exec {then}')
        script.write_text(f'#!/bin/sh\n{blue_shell}\n')
        script.chmod(0o755)
        # Tabs carry a star in their left padding: drags start on the label, and the config is private.
        config = CHECKS / f'split-config-{os.getpid()}'
        self.addCleanup(shutil.rmtree, config, ignore_errors=True)
        env.update(GDK_BACKEND='x11', SHELL=str(script), XDG_CONFIG_HOME=str(config))
        env.pop('WAYLAND_DISPLAY', None)
        app = subprocess.Popen([str(runtime_binary("worminal-rust")), '-T', 'Rust split', '-o', '-',
                                '-e', '/bin/sh', '-c', fill(red, 'red')],
                               env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.addCleanup(lambda: self.stop(app))
        wid = window(env, app)
        xdo = lambda *args: subprocess.run(['xdotool', *args], env=env, check=True)
        api = lambda *args: subprocess.check_output([str(runtime_binary('worminald')), 'api', *args],
                                                    env=env, text=True)
        server = display.Display(env['DISPLAY'])
        cleanup.callback(server.close)
        root = server.create_resource_object('window', int(wid))

        size = lambda: window_size(root)
        self.assertEqual(fix_size(root, xdo, wid, 900, 500), (900, 500))

        def pixel(fx, fy):
            try:
                data = image_bytes(root.get_image(int(size()[0] * fx), int(size()[1] * fy), 1, 1,
                                                  X.ZPixmap, 0xffffffff))
            except error.BadMatch:  # not mapped yet
                return None
            return tuple(data[:3][::-1])

        def wait_for(left, right, message):
            deadline = time.monotonic() + 8
            while (pixel(.25, .75), pixel(.75, .75)) != (left, right):
                self.assertIsNone(app.poll())
                if time.monotonic() >= deadline:
                    width, height = size()
                    data = image_bytes(root.get_image(0, 0, width, height, X.ZPixmap, 0xffffffff))
                    Image.frombuffer('RGB', (width, height), data, 'raw', 'BGRX', 0, 1).save(
                        CHECKS / 'rust-app-split.png')
                    self.fail(f'{message}: {pixel(.25, .75)} {pixel(.75, .75)}')
                time.sleep(.05)

        wait_for(red, red, 'first tab did not fill the window')
        xdo('windowfocus', '--sync', wid)
        xdo('key', '--clearmodifiers', 'ctrl+shift+t')
        deadline = time.monotonic() + 5
        while len(json.loads(api('list'))['tabs']) != 2:
            self.assertLess(time.monotonic(), deadline, 'second tab did not open')
            time.sleep(.05)
        # The new tab is selected and fills the window.
        wait_for(blue, blue, 'second tab did not fill the window')
        return SimpleNamespace(app=app, wid=wid, xdo=xdo, api=api, root=root, size=size, pixel=pixel,
                               wait_for=wait_for, trigger=trigger.touch)

    def test_dragging_a_tab_onto_a_pane_edge_splits_the_window(self):
        from Xlib import X
        from PIL import Image
        red, blue = self.RED, self.BLUE
        with isolated_display() as env, ExitStack() as cleanup:
            window = self.colored_split_window(env, cleanup)
            app, wid, xdo, api, root = window.app, window.wid, window.xdo, window.api, window.root
            size, pixel, wait_for = window.size, window.pixel, window.wait_for
            data = image_bytes(root.get_image(0, 0, 900, 500, X.ZPixmap, 0xffffffff))
            Image.frombuffer('RGB', (900, 500), data, 'raw', 'BGRX', 0, 1).save(CHECKS / 'split-before.png')
            # Locate the selected second label rather than assuming a particular font
            # size or directory basename width. Its right edge must clear the first label.
            background = pixel(.49, 4 / 500)
            second_label = max(x for x in range(2, 440) if pixel(x / 900, 4 / 500) != background) - 2
            self.assertLess(second_label, 450, 'fixture labels must fit in one half-width strip')
            # Drag the first tab's label onto the right edge of the pane.
            xdo('mousemove', '--window', wid, '40', '8', 'mousedown', '1')
            for step in range(1, 11):
                xdo('mousemove', '--window', wid, str(40 + 848 * step // 10), str(8 + 250 * step // 10))
                time.sleep(.02)
            deadline = time.monotonic() + 3
            while pixel(.75, .75) == blue:
                self.assertLess(time.monotonic(), deadline, 'split target highlight did not paint over content')
                time.sleep(.02)
            self.assertEqual(pixel(.25, .75), blue, 'split target highlight covered the wrong half')
            # A translucent copy of the tab follows the pointer (at 888, 258), held where it was grabbed: just
            # left of the pointer lies the tab's padding, which differs from the highlighted content around it.
            under = lambda: pixel(880 / 900, 258 / 500)
            marked = pixel(.75, .75)
            deadline = time.monotonic() + 3
            while under() == marked:
                self.assertLess(time.monotonic(), deadline, 'no dragged tab image under the pointer')
                time.sleep(.02)
            self.assertNotEqual(pixel(880 / 900, 300 / 500), under(), 'the dragged tab image is too tall')
            xdo('mouseup', '1')
            # The anchor stays left; the dropped tab takes the right half and focus.
            wait_for(blue, red, 'dropping a tab on the right edge did not split the window')
            self.assertEqual(under(), red, 'the dragged tab image outlived the drop')
            # Split panes are outlined, the focused one (right, just dropped) brighter than the other. Look at the
            # inner edges: the window's own outer frame is drawn by the display, not by the panes.
            left_edge, right_edge = pixel(449.5 / 900, .75), pixel(451.5 / 900, .75)
            self.assertNotEqual(left_edge, blue, 'the left pane has no border')
            self.assertNotEqual(right_edge, red, 'the right pane has no border')
            self.assertGreater(sum(right_edge), sum(left_edge), f'the focused border is not brighter: {left_edge} {right_edge}')
            # Clicking the other pane makes its tab the current one, not just the target of typing: Ctrl+Shift+C/V
            # and the window title follow the app's active tab, which a click on an already-live pane once left alone.
            title = lambda: subprocess.check_output(['xdotool', 'getwindowname', wid], env=env, text=True).strip()
            self.assertEqual(title(), 'red')
            xdo('mousemove', '--window', wid, '225', '300', 'click', '1')
            deadline = time.monotonic() + 5
            while title() != 'blue':
                self.assertLess(time.monotonic(), deadline, f'clicking the left pane left {title()!r} current')
                time.sleep(.05)
            xdo('mousemove', '--window', wid, '675', '300', 'click', '1')
            deadline = time.monotonic() + 5
            while title() != 'red':
                self.assertLess(time.monotonic(), deadline, f'clicking the right pane left {title()!r} current')
                time.sleep(.05)
            # Each pane has its own strip holding only its tabs: the dropped tab is selected in the right strip,
            # and the left strip no longer has the second tab where the shared strip had it.
            self.assertNotEqual(pixel(.505, 4 / 500), background, 'the right pane has no tab strip of its own')
            self.assertEqual(pixel(second_label / 900, 4 / 500), background, 'the left strip still lists the dropped tab')
            # Dragging the right pane's tab onto the left strip puts it back there and ends the split. Over a strip
            # the drop is marked by the yellow slot line there, never by the pane highlight, which is for content.
            # The press stays right of the tab's star, which would bookmark instead of drag. The pointer ends near
            # the strip's top, so the tab image it drags leaves the line's lower end clear, and over the left half of the strip's tab, so the dropped tab goes before it and stays first.
            content = next(y for y in range(500) if pixel(.25, y / 500) == blue)
            xdo('mousemove', '--window', wid, '490', '8', 'mousedown', '1')
            for step in range(1, 11):
                xdo('mousemove', '--window', wid, str(490 - 484 * step // 10), str(8 - 5 * step // 10))
                time.sleep(.02)
            yellow = bytes.fromhex(json.loads((ROOT / 'assets/palette.json').read_text())['normal']['yellow'][1:])
            strip = lambda: image_bytes(root.get_image(0, 0, 450, content, X.ZPixmap, 0xffffffff))
            deadline = time.monotonic() + 3
            while not any(bytes(data[i:i + 3][::-1]) == yellow for data in [strip()] for i in range(0, len(data), 4)):
                self.assertLess(time.monotonic(), deadline, 'no slot marker in the strip the tab is over')
                time.sleep(.02)
            self.assertEqual(pixel(.25, .75), blue, 'a drag over a strip highlighted the pane')
            xdo('mouseup', '1')
            wait_for(red, red, 'dropping a tab on the other strip did not merge the panes')
            self.assertEqual(pixel(.505, 4 / 500), background, 'the right strip outlived the split')
            # Split again, then Ctrl+Shift+X gathers the tabs.
            xdo('mousemove', '--window', wid, '40', '8', 'mousedown', '1')
            for step in range(1, 11):
                xdo('mousemove', '--window', wid, str(40 + 848 * step // 10), str(8 + 250 * step // 10))
                time.sleep(.02)
            xdo('mouseup', '1')
            wait_for(blue, red, 'the second split did not happen')
            xdo('key', '--clearmodifiers', 'ctrl+shift+x')
            wait_for(red, red, 'ctrl+shift+x did not leave only the focused pane')
            self.assertEqual(pixel(.505, 4 / 500), background, 'the right strip outlived the unsplit')
            # The keyboard does what dragging does. At the window's edge Alt+Shift+Right splits the shown
            # tab off to the right (the first tab is red and was the one selected by the gather) ...
            xdo('key', '--clearmodifiers', 'alt+shift+Right')
            wait_for(blue, red, 'alt+shift+Right did not split the tab off to the right')
            # ... Ctrl+Shift+Left focuses the left pane without moving anything ...
            xdo('key', '--clearmodifiers', 'ctrl+shift+Left')
            time.sleep(.3)
            wait_for(blue, red, 'ctrl+shift+Left moved a tab')
            # ... and with a pane to the right Alt+Shift+Right moves the tab into it, ending the split.
            xdo('key', '--clearmodifiers', 'alt+shift+Right')
            wait_for(blue, blue, 'alt+shift+Right did not move the tab into the right pane')
            self.assertEqual(pixel(.505, 4 / 500), background, 'the emptied pane outlived the move')

    def test_a_dragged_divider_stays_when_a_tab_moves_between_panes(self):
        # Moving a tab rebuilds the pane widgets; their dividers once restarted in the middle every time.
        red, blue = self.RED, self.BLUE
        with isolated_display() as env, ExitStack() as cleanup:
            window = self.colored_split_window(env, cleanup)
            xdo, api, pixel = window.xdo, window.api, window.pixel
            xdo('key', '--clearmodifiers', 'ctrl+shift+t')
            deadline = time.monotonic() + 5
            while len(json.loads(api('list'))['tabs']) != 3:
                self.assertLess(time.monotonic(), deadline, 'third tab did not open')
                time.sleep(.05)
            xdo('key', '--clearmodifiers', 'alt+shift+Right')

            def divider():
                # The middle of the pane outlines around the handle between the colored panes, in window pixels.
                row = [pixel(x / 900, .75) for x in range(100, 800)]
                edges = [100 + x for x, color in enumerate(row) if color not in (red, blue, None)]
                return (edges[0] + edges[-1]) // 2 if edges and edges[-1] - edges[0] < 12 else None
            deadline = time.monotonic() + 5
            while divider() is None or abs(divider() - 450) > 8:
                self.assertLess(time.monotonic(), deadline, f'alt+shift+Right did not split evenly: {divider()}')
                time.sleep(.05)
            start = divider()
            # The handle is the dark line just right of the outlines' middle.
            xdo('mousemove', '--window', window.wid, str(start + 1), '300', 'mousedown', '1')
            for x in range(start, 249, -20):
                xdo('mousemove', '--window', window.wid, str(x), '300')
                time.sleep(.02)
            xdo('mousemove', '--window', window.wid, '250', '300', 'mouseup', '1')
            deadline = time.monotonic() + 5
            while divider() is None or abs(divider() - 250) > 10:
                self.assertLess(time.monotonic(), deadline, f'the divider was not dragged: {divider()}')
                time.sleep(.05)
            dragged = divider()
            # The left pane keeps a tab, so moving its other one right changes no split.
            xdo('key', '--clearmodifiers', 'ctrl+shift+Left')
            time.sleep(.3)
            xdo('key', '--clearmodifiers', 'alt+shift+Right')
            time.sleep(1)
            self.assertEqual(len(json.loads(api('list'))['tabs']), 3)
            self.assertIsNotNone(divider(), 'moving the tab ended the split')
            self.assertLess(abs(divider() - dragged), 4, f'moving a tab reset the divider from {dragged} to {divider()}')

    def test_a_split_browser_keeps_its_pane_and_takes_new_tabs_when_clicked(self):
        red, blue, green = self.RED, self.BLUE, (20, 160, 60)
        page = CHECKS / 'split-browser.html'
        page.write_text('<!doctype html><body style="background:#14a03c"></body>')
        replace = f"{shlex.quote(str(runtime_binary('worminald')))} --new-browser-tab {shlex.quote(page.as_uri())}"
        with isolated_display() as env, profile(env) as env, ExitStack() as cleanup:
            window = self.colored_split_window(env, cleanup, then=replace)
            xdo, wid, wait_for = window.xdo, window.wid, window.wait_for
            # Drag the first (red) tab's label, right of its star, onto the right edge.
            xdo('mousemove', '--window', wid, '40', '8', 'mousedown', '1')
            for step in range(1, 11):
                xdo('mousemove', '--window', wid, str(40 + 848 * step // 10), str(8 + 250 * step // 10))
                time.sleep(.02)
            xdo('mouseup', '1')
            wait_for(blue, red, 'dropping a tab on the right edge did not split the window')
            # The unfocused left terminal replaces itself with a browser, as `worminal --new-browser-tab`. The
            # browser keeps that pane rather than joining the focused one, which would collapse the split.
            window.trigger()
            wait_for(green, red, 'the left terminal did not become a browser in its own pane')
            # A browser consumes its clicks and only takes keyboard focus; the pane must still become current.
            xdo('mousemove', '--window', wid, '675', '375', 'click', '1')
            xdo('mousemove', '--window', wid, '225', '375', 'click', '1')
            time.sleep(.3)
            xdo('key', '--clearmodifiers', 'ctrl+shift+t')
            wait_for(blue, red, 'Ctrl+Shift+T after clicking the browser pane opened the tab elsewhere')

    def test_a_browser_tab_moves_into_a_split_keeping_its_page_and_the_terminal_painting(self):
        from Xlib import X, display, error
        red, green, white = (200, 30, 30), (30, 160, 30), (255, 255, 255)
        with isolated_display() as env, ExitStack() as cleanup:
            root = CHECKS / f'browser-split-{os.getpid()}'
            shutil.rmtree(root, ignore_errors=True)
            root.mkdir(parents=True)
            self.addCleanup(shutil.rmtree, root, ignore_errors=True)
            # The title is random per load, so a reloaded page (a lost DOM) shows a different one.
            page = root / 'page.html'
            page.write_text('<!doctype html><title>t</title><body style="margin:0;background:#fff">'
                            '<script>document.title="doc-" + Math.floor(Math.random() * 1e9)</script>')
            # A terminal that changes color every quarter second shows whether its frames still arrive:
            # a hidden tab would freeze on whichever color it last painted.
            flip = ("while :; do printf '\\033[48;2;200;30;30m\\033[2J'; sleep .25; "
                    "printf '\\033[48;2;30;160;30m\\033[2J'; sleep .25; done")
            env.update(GDK_BACKEND='x11', XDG_CONFIG_HOME=str(root / 'config'))
            env.pop('WAYLAND_DISPLAY', None)
            app = subprocess.Popen([str(runtime_binary('worminal-rust')), '-T', 'Rust browser split', '-o', '-',
                                    '-e', '/bin/sh', '-c', flip],
                                   env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            self.addCleanup(lambda: self.stop(app))
            wid = window(env, app)
            xdo = lambda *args: subprocess.run(['xdotool', *args], env=env, check=True)
            api = lambda *args: subprocess.check_output([str(runtime_binary('worminald')), 'api', *args],
                                                        env=env, text=True)
            title = lambda: subprocess.check_output(['xdotool', 'getwindowname', wid], env=env, text=True).strip()
            server = display.Display(env['DISPLAY'])
            cleanup.callback(server.close)
            window_object = server.create_resource_object('window', int(wid))
            xdo('windowfocus', '--sync', wid)
            self.assertEqual(fix_size(window_object, xdo, wid, 900, 500), (900, 500))

            def pixel(fx, fy):
                try:
                    data = image_bytes(window_object.get_image(int(900 * fx), int(500 * fy), 1, 1, X.ZPixmap, 0xffffffff))
                except error.BadMatch:
                    return None
                return tuple(data[:3][::-1])

            def wait_for(condition, message):
                deadline = time.monotonic() + 10
                while not condition():
                    self.assertIsNone(app.poll())
                    self.assertLess(time.monotonic(), deadline, message)
                    time.sleep(.05)
            tabs = lambda: json.loads(api('list'))['tabs']
            api('open', '--browser', page.as_uri())
            wait_for(lambda: len(tabs()) == 2 and tabs()[1]['kind'] == 'browser', 'no browser tab appeared')
            xdo('key', '--clearmodifiers', 'alt+2')
            wait_for(lambda: pixel(.5, .75) == white, 'the browser page was not shown')
            wait_for(lambda: title().startswith('doc-'), 'the page never reported its title')
            loaded = title()
            # Alt+Shift+Right moves the browser tab into a pane of its own, beside the terminal.
            xdo('key', '--clearmodifiers', 'alt+shift+Right')
            wait_for(lambda: pixel(.75, .75) == white and pixel(.25, .75) in (red, green),
                     'the browser tab did not split off beside the terminal')
            # The page moved with its widget: it was not reloaded.
            time.sleep(2)
            self.assertEqual(title(), loaded, 'moving the browser tab reloaded its page')
            # The terminal beside the browser still paints: both of its colors keep arriving.
            seen = set()
            deadline = time.monotonic() + 4
            while time.monotonic() < deadline and not {red, green} <= seen:
                seen.add(pixel(.25, .75))
                time.sleep(.05)
            self.assertTrue({red, green} <= seen,
                            f'the terminal beside the browser stopped painting: saw {seen}')
            self.assertEqual(pixel(.75, .75), white)
            # Right after the split the terminal's own resize keeps it shown, which hides a fault: selecting the
            # browser pane again later must not hide the terminal, as an exclusive Focus on the browser would.
            # Ctrl+Alt+arrows select panes like Ctrl+Shift+arrows.
            xdo('key', '--clearmodifiers', 'ctrl+alt+Left')
            wait_for(lambda: title() != loaded, 'ctrl+alt+Left did not select the terminal pane')
            time.sleep(1)
            xdo('key', '--clearmodifiers', 'ctrl+alt+Right')
            wait_for(lambda: title() == loaded, 'the browser pane was not selected again')
            time.sleep(1)
            seen = set()
            deadline = time.monotonic() + 4
            while time.monotonic() < deadline and not {red, green} <= seen:
                seen.add(pixel(.25, .75))
                time.sleep(.05)
            self.assertTrue({red, green} <= seen,
                            f'selecting the browser pane hid the terminal beside it: saw {seen}')

    def test_tab_strips_rewrap_when_splits_change_their_width(self):
        from Xlib import X, display
        red, blue = (200, 30, 30), (20, 40, 210)
        fill = lambda color: ("while :; do printf '\\033[48;2;%d;%d;%dm\\033[2J'; sleep .2; done" % color)
        # Labels are the directory basename; at 30 cells two fit a 900px row (one of them bold),
        # but only one fits half of it.
        directory = CHECKS / ('w' * 30)
        directory.mkdir(exist_ok=True)
        with isolated_display() as env, ExitStack() as cleanup:
            script = CHECKS / 'rewrap-shell.sh'
            script.write_text(f'#!/bin/sh\ncd {shlex.quote(str(directory))}\n{fill(blue)}\n')
            script.chmod(0o755)
            env.update(GDK_BACKEND='x11', SHELL=str(script))
            env.pop('WAYLAND_DISPLAY', None)
            app = subprocess.Popen([str(runtime_binary("worminal-rust")), '-T', 'Rust rewrap', '-o', '-',
                                    '-e', '/bin/sh', '-c', fill(red)], cwd=directory,
                                   env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            self.addCleanup(lambda: self.stop(app))
            wid = window(env, app)
            xdo = lambda *args: subprocess.run(['xdotool', *args], env=env, check=True)
            api = lambda *args: subprocess.check_output([str(runtime_binary('worminald')), 'api', *args],
                                                        env=env, text=True)
            server = display.Display(env['DISPLAY'])
            cleanup.callback(server.close)
            root = server.create_resource_object('window', int(wid))
            self.assertEqual(fix_size(root, xdo, wid, 900, 500), (900, 500))

            def strip_height(x):
                # The strip ends where the terminal's own fill begins.
                data = image_bytes(root.get_image(x, 0, 1, 500, X.ZPixmap, 0xffffffff))
                column = [tuple(data[4 * y:4 * y + 3][::-1]) for y in range(500)]
                return next((y for y, color in enumerate(column) if color in (red, blue)), None)

            def fail(message):
                from PIL import Image
                data = image_bytes(root.get_image(0, 0, 900, 500, X.ZPixmap, 0xffffffff))
                Image.frombuffer('RGB', (900, 500), data, 'raw', 'BGRX', 0, 1).save(CHECKS / 'rust-app-rewrap.png')
                self.fail(message)

            def wait_height(x, expected, message):
                deadline = time.monotonic() + 8
                while strip_height(x) != expected:
                    self.assertIsNone(app.poll())
                    if time.monotonic() >= deadline:
                        fail(f'{message}: {strip_height(x)} != {expected}')
                    time.sleep(.05)

            def settle(x):
                # Strip rows are whatever the font makes them, so heights are compared once they stop changing.
                last, since, deadline = None, time.monotonic(), time.monotonic() + 8
                while time.monotonic() - since < .5:
                    self.assertLess(time.monotonic(), deadline, 'the strip height did not settle')
                    if strip_height(x) != last:
                        last, since = strip_height(x), time.monotonic()
                    time.sleep(.05)
                return last

            one = settle(225)
            self.assertIsNotNone(one, 'first tab did not paint')
            xdo('windowfocus', '--sync', wid)
            for count in (2, 3, 4):
                xdo('key', '--clearmodifiers', 'ctrl+shift+t')
                deadline = time.monotonic() + 5
                while len(json.loads(api('list'))['tabs']) != count:
                    self.assertLess(time.monotonic(), deadline, f'tab {count} did not open')
                    time.sleep(.05)
            # New labels follow each shell's directory once the daemon reports it.
            deadline = time.monotonic() + 8
            while strip_height(225) in (one, None):
                if time.monotonic() >= deadline:
                    fail(f'four tabs did not wrap onto a second row: {strip_height(225)} at {window_size(root)}')
                time.sleep(.05)
            row = settle(225) - one
            # Splitting off the selected tab leaves three in a half-width strip: one per row.
            xdo('key', '--clearmodifiers', 'alt+shift+Right')
            wait_height(225, one + 2 * row, 'the narrowed strip did not wrap onto three rows')
            wait_height(675, one, 'the new pane strip is not one row')
            # Gathering the tabs widens the strip again.
            xdo('key', '--clearmodifiers', 'ctrl+shift+x')
            wait_height(225, one + row, 'the widened strip did not unwrap to two rows')
            wait_height(675, one + row, 'the widened strip did not span the window')

    @staticmethod
    def stop(process):
        stop_application(process)
