"""Regenerate silent, tiny media regression fixtures through manage.py media-fixtures."""
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent.parent


def main():
    directory = ROOT / "tests/fixtures/media"
    directory.mkdir(parents=True, exist_ok=True)
    audio = ["-f", "lavfi", "-i", "anullsrc=r=44100:cl=mono"]
    variants = {
        "sample.mp3": [*audio, "-c:a", "libmp3lame", "-b:a", "64k"],
        "sample.wav": [*audio, "-ar", "8000", "-c:a", "pcm_s16le"],
        "sample.mp4": ["-f", "lavfi", "-i", "color=c=red:size=96x64:rate=10", *audio,
                       "-c:v", "libx264", "-profile:v", "baseline", "-pix_fmt", "yuv420p",
                       "-c:a", "aac", "-movflags", "+faststart"],
    }
    for name, options in variants.items():
        subprocess.run(["ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-y",
                        *options, "-t", "3", "-map_metadata", "-1", str(directory / name)], check=True)
    print("Generated silent MP3, WAV and H.264/AAC MP4 fixtures.")


if __name__ == "__main__":
    main()
