"""Interpret staged paths consistently for repository content checks."""

import os


def tracked_files(index):
    """Yield file paths from git ls-files --stage -z; gitlinks own separate repositories."""
    for record in index.split(b"\0"):
        if not record:
            continue
        metadata, path = record.split(b"\t", 1)
        if metadata.split(b" ", 1)[0] != b"160000":
            yield os.fsdecode(path)
