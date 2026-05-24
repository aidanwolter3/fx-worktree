# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import re
import sys
from PIL import Image, ImageDraw, ImageFont

class Terminal:
    def __init__(self, width=800, height=450, font_path="/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", font_size=14):
        self.width = width
        self.height = height
        self.font = ImageFont.truetype(font_path, font_size)
        self.lines = []
        self.max_lines = 20
        self.line_height = 22
        self.padding = 15
        self.bg_color = (30, 30, 30) # #1e1e1e
        self.fg_color = (212, 212, 212) # #d4d4d4
        self.colors = {
            'g': (78, 201, 176),  # Green
            'b': (86, 156, 214),  # Blue
            'y': (206, 145, 120), # Yellow/Orange
            'w': (212, 212, 212), # White/Gray
        }

    def add_line(self, line):
        self.lines.append(line)
        if len(self.lines) > self.max_lines:
            self.lines.pop(0)

    def update_last_line(self, line):
        if self.lines:
            self.lines[-1] = line
        else:
            self.lines.append(line)

    def strip_tags(self, line):
        return re.sub(r'\[\[[gbyw]\]\]', '', line)

    def draw(self, show_cursor=True):
        img = Image.new('RGB', (self.width, self.height), color=self.bg_color)
        draw = ImageDraw.Draw(img)
        
        for i, line in enumerate(self.lines):
            y = self.padding + i * self.line_height
            self.draw_line(draw, line, self.padding, y)
            
        # Draw cursor at the end of the last line
        if show_cursor and self.lines:
            last_line = self.lines[-1]
            raw_text = self.strip_tags(last_line)
            x = self.padding + draw.textlength(raw_text, font=self.font)
            y = self.padding + (len(self.lines) - 1) * self.line_height
            draw.rectangle([x + 2, y + 4, x + 10, y + self.line_height - 2], fill=self.fg_color)
            
        return img

    def draw_line(self, draw, line, x, y):
        parts = re.split(r'(\[\[[gbyw]\]\])', line)
        current_color = self.fg_color
        current_x = x
        for part in parts:
            if not part:
                continue
            if part.startswith('[[') and part.endswith(']]'):
                color_code = part[2]
                current_color = self.colors.get(color_code, self.fg_color)
            else:
                draw.text((current_x, y), part, font=self.font, fill=current_color)
                current_x += draw.textlength(part, font=self.font)

def generate_frames():
    term = Terminal()
    frames = []

    def append_frame(show_cursor=True, duration=100):
        frames.append((term.draw(show_cursor), duration))

    def type_command(cmd):
        prompt = "[[g]]❯[[w]] "
        term.add_line(prompt)
        append_frame(show_cursor=True, duration=300)
        
        for i in range(len(cmd) + 1):
            typed = cmd[:i]
            term.update_last_line(prompt + typed)
            append_frame(show_cursor=True, duration=80)
        
        for _ in range(2):
            append_frame(show_cursor=False, duration=250)
            append_frame(show_cursor=True, duration=250)

    # 1. fx-worktree list
    type_command("fx-worktree list")
    term.add_line("CONFIG        WORKTREE ID            STATUS")
    term.add_line("[[b]]fuchsia.x64[[w]]   fuchsia.x64-37954053   [[g]]Free[[w]]")
    append_frame(show_cursor=True, duration=1500)

    # 2. fx-worktree lease fuchsia.x64
    type_command("fx-worktree lease fuchsia.x64")
    term.add_line("[[g]]✔ Worktree leased successfully![[w]]")
    term.add_line("")
    term.add_line("  Worktree ID  : [[b]]fuchsia.x64-d704c897[[w]]")
    term.add_line("  Agent ID     : agent-2f26359d")
    term.add_line("  Config       : [[b]]fuchsia.x64[[w]]")
    term.add_line("  Path         : [[b]]/usr/local/google/home/username/fuchsia/.jiri_root/worktrees/fuchsia.x64-d704c897[[w]]")
    term.add_line("")
    term.add_line("To change directory into the worktree:")
    term.add_line("  $ fx-worktree cd [[b]]fuchsia.x64-d704c897[[w]]  # Navigate to this specific worktree")
    term.add_line("  $ fx-worktree cd                     # Navigate to the last leased worktree")
    append_frame(show_cursor=True, duration=3000)

    # 3. fx-worktree list again
    type_command("fx-worktree list")
    term.add_line("CONFIG        WORKTREE ID            STATUS")
    term.add_line("[[b]]fuchsia.x64[[w]]   fuchsia.x64-37954053   [[g]]Free[[w]]")
    term.add_line("[[b]]fuchsia.x64[[w]]   fuchsia.x64-d704c897   [[y]]In Use (agent-2f26359d)[[w]]")
    append_frame(show_cursor=True, duration=2000)

    # 4. fx-worktree release fuchsia.x64-d704c897
    type_command("fx-worktree release fuchsia.x64-d704c897")
    term.add_line("Resetting worktree [[b]]fuchsia.x64-d704c897[[w]]...")
    term.add_line("[[g]]✔ Worktree fuchsia.x64-d704c897 successfully released.[[w]]")
    append_frame(show_cursor=True, duration=2000)

    # Final blink
    for _ in range(2):
        term.add_line("[[g]]❯[[w]] ")
        append_frame(show_cursor=True, duration=250)
        term.update_last_line("[[g]]❯[[w]]")
        append_frame(show_cursor=False, duration=250)

    return frames

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: python3 generate_demo.py <output_path>")
        sys.exit(1)
    output_path = sys.argv[1]
    os.makedirs(os.path.dirname(output_path), exist_ok=True)
    
    print("Generating frames...")
    frames_data = generate_frames()
    
    images = [f[0] for f in frames_data]
    durations = [f[1] for f in frames_data]
    
    print(f"Saving GIF to {output_path}...")
    images[0].save(
        output_path,
        save_all=True,
        append_images=images[1:],
        duration=durations,
        loop=0
    )
    print("Done!")
