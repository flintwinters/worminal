"""Minimal raw PTY echo child for the visible latency benchmark."""

import os
import tty


tty.setraw(0)
os.write(1, b"\033[?25l\033[48;2;17;34;51m\033[2J\033[H")
while data := os.read(0, 1):
    os.write(1, b"\033[12;40H" + data)
