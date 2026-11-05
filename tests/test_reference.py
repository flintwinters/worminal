"""Test extraction of old C code and cleanup of temporary test files."""
import io
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from tools import reference


class HistoricalReferenceTests(unittest.TestCase):
    def setUp(self):
        (reference.ROOT / 'build/checks').mkdir(exist_ok=True)
        self.directory = tempfile.TemporaryDirectory(prefix='reference-', dir=reference.ROOT / 'build/checks')
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)

    def test_reference_is_reproducible_from_git_not_a_saved_release(self):
        reference.extract(reference.REVISION, self.root)
        self.assertTrue((self.root / 'src/gtk/app.c').is_file())
        self.assertTrue((self.root / 'src/terminal/engine.c').is_file())
        self.assertFalse((self.root / 'worminal').exists())
        self.assertFalse((self.root / 'worminald').exists())

    def test_engine_oracle_baseline_is_available_in_repository_history(self):
        from tools.engine_checks import BASELINE
        reference.extract(BASELINE, self.root)
        self.assertTrue((self.root / 'src/terminal/st.c').is_file())
        self.assertTrue((self.root / 'src/terminal/st_state.h').is_file())
        self.assertFalse((self.root / 'src/terminal/engine.c').exists())

    def test_explicit_source_paths_support_isolated_rust_performance_baselines(self):
        reference.extract('HEAD', self.root, paths=('Cargo.toml', 'Cargo.lock'))
        self.assertTrue((self.root / 'Cargo.toml').is_file())
        self.assertTrue((self.root / 'Cargo.lock').is_file())
        self.assertFalse((self.root / 'src').exists())

    def test_archive_cannot_escape_its_disposable_directory(self):
        output = io.BytesIO()
        with tarfile.open(fileobj=output, mode='w') as archive:
            entry = tarfile.TarInfo('../escaped')
            entry.size = 1
            archive.addfile(entry, io.BytesIO(b'x'))
        with patch.object(reference.subprocess, 'check_output', return_value=output.getvalue()):
            with self.assertRaisesRegex(ValueError, 'Unsafe historical fixture path'):
                reference.extract(reference.REVISION, self.root / 'fixture')
        self.assertFalse((self.root / 'escaped').exists())

    def test_fixture_target_cannot_replace_current_product_binaries(self):
        with patch.object(reference, 'module') as historical:
            for name in ('worminal', 'worminald', 'worminal-rust', 'worminal-preview', 'worminald-preview'):
                with self.assertRaisesRegex(ValueError, 'cannot replace current application'):
                    reference.target(name)
            historical.assert_not_called()

    def test_cleanup_removes_owned_links_but_preserves_replacements(self):
        original, replacement = self.root / 'old', self.root / 'new'
        original.touch(); replacement.touch()
        owned, changed = self.root / 'owned', self.root / 'changed'
        owned.symlink_to(original); changed.symlink_to(replacement)
        with (patch.object(reference, '_links', {owned: original, changed: original}),
              patch.object(reference, '_directory', None), patch.object(reference, '_module', None),
              patch.object(reference, '_parents', set())):
            reference.cleanup()
        self.assertFalse(owned.is_symlink())
        self.assertEqual(changed.readlink(), replacement)
        self.assertTrue(original.exists())
        self.assertTrue(replacement.exists())

    def test_clean_removes_interrupted_fixture_links_only(self):
        source = self.root / 'src'
        source.mkdir()
        stale = source / 'stale.c'
        stale.symlink_to(self.root / 'build/.historical-interrupted/src/stale.c')
        user = source / 'user.c'
        user.symlink_to(self.root / 'user-source.c')
        reference.remove_fixture_links(self.root)
        self.assertFalse(stale.is_symlink())
        self.assertTrue(user.is_symlink())
