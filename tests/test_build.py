"""Build ownership: dependency invalidation, failed compiles, staging, and cleanup."""

import os
import re
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from tools import build, weston


class BuildTests(unittest.TestCase):
    def setUp(self):
        checks = build.ROOT / "build/checks"
        checks.mkdir(parents=True, exist_ok=True)
        self.directory = tempfile.TemporaryDirectory(prefix="build-", dir=checks)
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.root_patch = patch.object(build, "ROOT", self.root)
        self.root_patch.start()
        self.addCleanup(self.root_patch.stop)

    def test_release_build_does_not_construct_historical_applications(self):
        with patch.object(build, "pkg_flags", return_value=[]), patch.object(build, "step"):
            builder = build.Builder()
            with (patch.object(builder, "program") as program, patch.object(builder, "rust_program") as rust,
                  patch.object(builder, "rust_library") as library, patch('tools.reference.target') as historical):
                for name in ("worminal", "worminald", "worminal-preview", "worminald-preview"):
                    builder.target(name)
                    rust.assert_called_with(name, "worminal-launcher" if name in
                                            ("worminal", "worminal-preview") else "worminal-daemon")
                program.assert_not_called()
                historical.assert_not_called()
                self.assertIn(unittest.mock.call("build/lib/worminal/" + build.RUST_FRONTEND_MODULE,
                                                "worminal-frontend"),
                              library.call_args_list)
                self.assertIn(unittest.mock.call("build/lib/worminal/" + build.RUST_BROWSER_MODULE, "worminal-browser"),
                              library.call_args_list)

    def test_headless_target_builds_and_installs_only_the_daemon(self):
        names = lambda: {destination.name for _, destination, _ in build.installation_files("/usr/local")}
        with patch.dict(os.environ, {"WORMINAL_HEADLESS": "1"}):
            self.assertEqual(names(), {"worminald", "worminal.1"})
            with patch.object(build, "generate_theme"), patch.object(build, "Builder") as builder:
                build.build_targets()
                self.assertEqual([c.args[0] for c in builder().target.call_args_list], ["worminald"])
        with patch.dict(os.environ, {"WORMINAL_HEADLESS": "0"}):
            self.assertIn("worminal", names())
        with patch.dict(os.environ, {}, clear=False):
            os.environ.pop("WORMINAL_HEADLESS", None)
            with patch.object(build.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "pkg-config")):
                self.assertTrue(build.headless())

    def test_historical_c_targets_delegate_to_disposable_git_fixtures(self):
        with patch.object(build, "pkg_flags", return_value=[]), patch('tools.reference.target') as historical:
            builder = build.Builder()
            with patch.object(builder, 'program') as program:
                for name in ('worminal-x11', 'worminald-x11', 'build/worminald-c-v2',
                             'build/frontend.so', 'build/browser.so', 'build/worminal-gtk-c'):
                    builder.target(name)
                    historical.assert_called_with(name)
                program.assert_not_called()

    def test_first_rust_publication_creates_bin_and_module_directories(self):
        source = self.root / "compiled"
        source.write_bytes(b"verified artifact")
        with patch.object(build, "pkg_flags", return_value=[]):
            builder = build.Builder()
        for output, destination in (
            ("worminald", self.root / "build/bin/worminald"),
            ("build/lib/worminal/frontend.so", self.root / "build/lib/worminal/frontend.so"),
        ):
            self.assertFalse(destination.parent.exists())
            builder.publish_rust(output, source)
            self.assertEqual(destination.read_bytes(), source.read_bytes())
            self.assertTrue(destination.stat().st_mode & 0o111)

    def test_rust_publication_preserves_running_inodes_and_failed_builds(self):
        source = self.root / "build/cargo/release/worminal-daemon"
        source.parent.mkdir(parents=True)
        source.write_bytes(b"new daemon")
        output = self.root / "build/bin/worminald"
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(b"running daemon")
        with patch.object(build, "pkg_flags", return_value=[]):
            builder = build.Builder()
        with patch.object(build, "invoke") as cargo, patch.object(builder, "target") as target, \
                output.open("rb") as running:
            builder.rust_program("worminald", "worminal-daemon")
            target.assert_called_once_with("build/libworminal_engine.a")
            cargo.assert_called_once_with(["cargo", "build", "--locked", "--release", "--quiet",
                                           "-p", "worminal-daemon"])
            self.assertEqual(running.read(), b"running daemon")
            self.assertEqual(output.read_bytes(), b"new daemon")
            self.assertEqual(output.stat().st_mode & 0o777, 0o755)
            identity = output.stat().st_ino
            builder.rust_program("worminald", "worminal-daemon")
            self.assertEqual(output.stat().st_ino, identity)
            cargo.side_effect = subprocess.CalledProcessError(1, ["cargo"])
            source.write_bytes(b"failed artifact")
            with self.assertRaises(subprocess.CalledProcessError):
                builder.rust_program("worminald", "worminal-daemon")
            self.assertEqual(output.read_bytes(), b"new daemon")
            self.assertEqual(output.stat().st_ino, identity)
            self.assertFalse(output.with_name("worminald.pending").exists())

    def test_manage_detaches_from_the_desktop_display(self):
        import manage
        desktop = {name: "desktop" for name in manage.DESKTOP_VARIABLES}
        with patch.dict(os.environ, desktop):
            manage.root()
            self.assertFalse(set(manage.DESKTOP_VARIABLES) & set(os.environ))

    def test_defaults_and_environment_flags(self):
        with patch.dict(os.environ, {}, clear=True), patch.object(build, "pkg_flags", return_value=[]):
            self.assertIn("-O3", build.compile_flags())
            with patch.dict(os.environ, {"CFLAGS": "-O0 -g", "CPPFLAGS": '-DNAME="two words"'}):
                flags = build.compile_flags()
                self.assertNotIn("-O3", flags)
                self.assertIn("-O0", flags)
                self.assertIn("-DNAME=two words", flags)

    def test_incremental_headers_flags_and_failure_recovery(self):
        source = self.root / "source.c"
        header = self.root / "value.h"
        source.write_text('#include "value.h"\nint main(void) { return VALUE; }\n')
        header.write_text("#define VALUE 0\n")
        command = ["cc", "-c", "source.c"]
        with patch.object(build, "invoke", wraps=build.invoke) as invoked:
            build.step("output.o", command, ["source.c"], depfile=True)
            output = self.root / "output.o"
            original = output.read_bytes()
            build.step("output.o", command, ["source.c"], depfile=True)
            self.assertEqual(invoked.call_count, 1)
            # Set an explicit later timestamp so the regression does not rely on clock resolution.
            header.write_text("#define VALUE 1\n")
            later = output.stat().st_mtime_ns + 1_000_000
            os.utime(header, ns=(later, later))
            build.step("output.o", command, ["source.c"], depfile=True)
            self.assertEqual(invoked.call_count, 2)
            os.utime(header, ns=(1, 1))
            build.step("output.o", [*command, "-O2"], ["source.c"], depfile=True)
            self.assertEqual(invoked.call_count, 3)
            good = output.read_bytes()
            self.assertNotEqual(original, good)
            with patch.object(build, "invoke", side_effect=subprocess.CalledProcessError(1, command)):
                with self.assertRaises(subprocess.CalledProcessError):
                    build.step("output.o", [*command, "-invalid"], ["source.c"], depfile=True)
            self.assertEqual(output.read_bytes(), good)
            self.assertFalse(list(self.root.glob("*.pending")))
            build.step("output.o", [*command, "-O2"], ["source.c"], depfile=True)
            self.assertEqual(invoked.call_count, 3)

    def test_link_dependencies_relink_without_recompiling(self):
        (self.root / "module.c").write_text("int entry(void) { return 1; }\n")
        exports = self.root / "exports.map"
        exports.write_text("{ global: entry; local: *; };\n")
        with patch.object(build, "pkg_flags", return_value=[]):
            builder = build.Builder()
        with patch.object(build, "invoke", wraps=build.invoke) as invoked:
            def link():
                builder.program("module.so", ["module.c"], flags=["-fPIC"],
                                libs=["-shared", "-Wl,--version-script=exports.map"],
                                link_dependencies=["exports.map"])
            link()
            self.assertEqual(invoked.call_count, 2)
            link()
            self.assertEqual(invoked.call_count, 2)
            exports.write_text("{ global: entry; };\n")
            modified = (self.root / "module.so").stat().st_mtime_ns + 1
            os.utime(exports, ns=(modified, modified))
            link()
            self.assertEqual(invoked.call_count, 3)
            self.assertNotIn("-c", invoked.call_args.args[0])

    def test_staged_install_uninstall_and_modes(self):
        for source in ("build/bin/worminal", "build/bin/worminald",
                       "build/lib/worminal/" + build.RUST_BROWSER_MODULE,
                       "build/lib/worminal/" + build.RUST_FRONTEND_MODULE,
                       "docs/man/worminal.1",
                       "assets/worminal.desktop", "assets/worminal.svg",
                       *(str(build.FONT_DIRECTORY / name) for name in build.FONT_FILES)):
            path = self.root / source
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("VERSION\n")
        stage = self.root / "stage"
        installed = stage / "opt/worminal"
        (installed / "bin").mkdir(parents=True)
        previous = stage / "previous-client"
        previous.write_text("previous client")
        (installed / "bin/worminal").symlink_to(previous)
        module = installed / "lib/worminal" / build.RUST_BROWSER_MODULE
        module.parent.mkdir(parents=True)
        module.write_text("running module")
        legacy = installed / "lib/worminal/browser.so"
        legacy.write_text("legacy browser module")
        old_rust = legacy.with_name('browser-rust-v1.so')
        old_rust.write_text('live Rust generation')
        old_c = legacy.with_name('browser-v123.so')
        old_c.write_text('retired C ABI')
        old_binary = installed / 'bin/worminal-x11'
        old_binary.write_text('retired C executable')
        with patch.dict(os.environ, {"DESTDIR": str(stage), "MANPREFIX": "/manuals"}), \
                patch.object(build, "build_targets") as compile_build:
            with module.open() as running_module:
                build.install("/opt/worminal")
                self.assertEqual(running_module.read(), "running module")
            self.assertFalse(legacy.exists())
            self.assertFalse(old_c.exists())
            self.assertFalse(old_binary.exists())
            self.assertEqual(old_rust.read_text(), 'live Rust generation')
            self.assertFalse((installed / "bin/worminal").is_symlink())
            self.assertEqual(previous.read_text(), "previous client")
            compile_build.assert_called_once_with()
            compile_build.reset_mock()
            build.install("/opt/worminal", skip_build=True)
            compile_build.assert_not_called()
            files = build.installation_files("/opt/worminal")
            for name in build.FONT_FILES:
                self.assertEqual((installed / "share/fonts/worminal" / name).read_text(), "VERSION\n")
            self.assertLess(next(i for i, (source, _, _) in enumerate(files)
                                 if source.name == build.RUST_FRONTEND_MODULE),
                            next(i for i, (source, _, _) in enumerate(files) if source.name == "worminal"))
            for _, destination, mode in files:
                self.assertTrue(destination.is_relative_to(stage))
                self.assertEqual(destination.stat().st_mode & 0o777, mode)
            self.assertEqual((stage / "manuals/man1/worminal.1").read_text(), build.VERSION + "\n")
            keep = stage / "keep"
            keep.touch()
            build.uninstall("/opt/worminal")
            self.assertTrue(keep.exists())
            self.assertFalse(legacy.exists())
            self.assertTrue(all(not destination.exists() for _, destination, _ in files))

    def test_clean_preserves_diagnostics_and_development_environment(self):
        (self.root / "build").mkdir()
        (self.root / "build/object.o").touch()
        (self.root / "build/checks").mkdir(parents=True)
        (self.root / "build/checks/failure.log").touch()
        (self.root / 'build/venv').mkdir()
        (self.root / 'build/venv/pyvenv.cfg').write_text('development environment')
        outputs = ("worminal", "worminald", "worminal-x11", "worminald-x11",
                   "worminal-preview", "worminald-preview")
        generated = [name + suffix for name in outputs for suffix in ("", ".command.json", ".pending")]
        generated += ["st.o", "x.o"]
        for name in generated:
            (self.root / name).touch()
        (self.root / "config.h").write_text("user configuration")
        build.clean()
        self.assertEqual({p.name for p in (self.root / "build").iterdir()}, {"checks", "venv"})
        self.assertTrue(all(not (self.root / name).exists() for name in generated))
        self.assertEqual((self.root / "config.h").read_text(), "user configuration")
        self.assertTrue((self.root / "build/checks/failure.log").exists())
        self.assertEqual((self.root / 'build/venv/pyvenv.cfg').read_text(), 'development environment')

    def test_relative_install_prefix_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "absolute"):
            build.installation_files("relative")

    def test_default_install_uses_user_prefix(self):
        with patch.dict(os.environ, {}, clear=True), patch.object(Path, "home", return_value=self.root):
            files = build.installation_files()
        self.assertEqual(files[0][1], self.root / ".local/lib/worminal" / build.RUST_BROWSER_MODULE)
        self.assertIn(self.root / ".local/bin/worminal", [destination for _, destination, _ in files])

    def weston_runtime(self, major):
        local = self.root / "weston"
        runtime = local / f"libweston-{major}-0"
        paths = ["usr/bin/weston", f"usr/include/libweston-{major}/libweston/libweston.h",
                 "usr/lib/testarch/weston/libexec_weston.so.0",
                 f"usr/lib/testarch/libweston-{major}/headless-backend.so",
                 "usr/libexec/weston-desktop-shell" if major == 16 else "usr/lib/testarch/weston-desktop-shell"]
        for name in paths:
            path = runtime / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.touch()
        (local / "active-library").write_text(f"libweston-{major}-0\n")
        return local, runtime

    def test_weston_selects_matching_abi_and_shell_paths(self):
        for major in (10, 16):
            with self.subTest(major=major):
                local, runtime = self.weston_runtime(major)
                env = {"LD_LIBRARY_PATH": "/retained"}
                with (patch.object(weston, "LOCAL", local), patch.object(weston, "ROOT", self.root),
                      patch.object(build, "step") as step, patch.object(build, "pkg_flags", return_value=[])):
                    command = weston.command(env)
                self.assertEqual(command[0], str(runtime / "usr/bin/weston"))
                output, compiler, dependencies = step.call_args.args
                self.assertEqual(output.name, f"weston_seat-{major}.so")
                self.assertIn(f"-I{runtime / f'usr/include/libweston-{major}'}", compiler)
                self.assertIn(local / "active-library", dependencies)
                self.assertTrue(step.call_args.kwargs["depfile"])
                self.assertIn(str(runtime / f"usr/lib/testarch/libweston-{major}"), env["WESTON_MODULE_MAP"])
                self.assertTrue(env["LD_LIBRARY_PATH"].endswith(":/retained"))
                config = (local / f"weston-{major}.ini").read_text()
                client = runtime / ("usr/libexec" if major == 16 else "usr/lib/testarch")
                self.assertIn(f"client={client / 'weston-desktop-shell'}", config)
                self.assertIn(f"path={client / 'weston-keyboard'}", config)

    def test_weston_rejects_mixed_and_mismatched_headers(self):
        local, runtime = self.weston_runtime(16)
        wrong = runtime / "usr/include/libweston-10/libweston/libweston.h"
        wrong.parent.mkdir(parents=True)
        wrong.touch()
        with patch.object(weston, "LOCAL", local), patch.object(build, "step") as step:
            with self.assertRaisesRegex(ValueError, "mixed"):
                weston.command({})
            (runtime / "usr/include/libweston-16/libweston/libweston.h").unlink()
            with self.assertRaisesRegex(ValueError, "mismatched"):
                weston.command({})
            step.assert_not_called()

    def test_weston_failed_download_preserves_selection(self):
        local, _ = self.weston_runtime(10)
        with (patch.object(weston, "LOCAL", local),
              patch.object(weston.subprocess, "check_output", return_value=" Depends: libweston-16-0\n"),
              patch.object(weston.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "apt-get"))):
            with self.assertRaises(subprocess.CalledProcessError):
                weston.setup()
        self.assertEqual((local / "active-library").read_text(), "libweston-10-0\n")
        self.assertFalse(list(local.glob("download-*")))

    def test_weston_setup_excludes_stale_packages(self):
        local = self.root / "weston"
        stale = local / "packages/old-runtime.deb"
        stale.parent.mkdir(parents=True)
        stale.touch()
        extracted = []

        def run(command, **kwargs):
            if command[0] == "apt-get":
                self.assertIn("libweston-16-dev", command)
                for name in ("weston.deb", "runtime.deb", "headers.deb"):
                    (Path(kwargs["cwd"]) / name).touch()
            else:
                extracted.append(Path(command[2]).name)
                self.assertEqual(Path(command[3]), local / "libweston-16-0")

        with (patch.object(weston, "LOCAL", local),
              patch.object(weston.subprocess, "check_output", return_value=" Depends: libweston-16-0\n"),
              patch.object(weston.subprocess, "run", side_effect=run)):
            weston.setup()
        self.assertCountEqual(extracted, ("weston.deb", "runtime.deb", "headers.deb"))
        self.assertEqual((local / "active-library").read_text(), "libweston-16-0\n")
        self.assertFalse((local / "active-library.pending").exists())
        self.assertTrue(stale.exists())
