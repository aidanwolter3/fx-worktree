# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import re
import sys
import tempfile
import subprocess
import shutil
from PIL import Image, ImageDraw, ImageFont

class Terminal:
    def __init__(self, width=800, height=450, font_path="/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", font_size=14):
        self.width = width
        self.height = height
        self.font = ImageFont.truetype(font_path, font_size)
        bold_font_path = font_path.replace("DejaVuSansMono.ttf", "DejaVuSansMono-Bold.ttf")
        self.font_bold = ImageFont.truetype(bold_font_path, font_size)
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
        return re.sub(r'\x1b\[[0-9;]*m', '', line)

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
        parts = re.split(r'(\x1b\[[0-9;]*m)', line)
        current_color = self.fg_color
        current_font = self.font
        current_x = x
        for part in parts:
            if not part:
                continue
            if part.startswith('\x1b['):
                code = part[2:-1]
                if code == '0':
                    current_color = self.fg_color
                    current_font = self.font
                elif code == '1':
                    current_font = self.font_bold
                elif code == '32':
                    current_color = self.colors.get('g', self.fg_color)
                elif code == '33':
                    current_color = self.colors.get('y', self.fg_color)
                elif code == '34':
                    current_color = self.colors.get('b', self.fg_color)
            else:
                draw.text((current_x, y), part, font=current_font, fill=current_color)
                current_x += draw.textlength(part, font=current_font)

def setup_mock_workspace():
    test_dir = tempfile.mkdtemp(prefix="fx-worktree-demo-")
    fuchsia_dir = os.path.join(test_dir, "fuchsia")
    os.makedirs(fuchsia_dir)
    
    # Create .jiri_root
    jiri_root = os.path.join(fuchsia_dir, ".jiri_root")
    os.makedirs(jiri_root)
    os.makedirs(os.path.join(jiri_root, "worktrees"))
    
    # Create empty worktrees_registry
    with open(os.path.join(jiri_root, "worktrees_registry"), "w") as f:
        pass
            
    # Create mock jiri script in parent .jiri_root/bin/jiri
    mock_jiri_dir = os.path.join(jiri_root, "bin")
    os.makedirs(mock_jiri_dir)
    mock_jiri_path = os.path.join(mock_jiri_dir, "jiri")
    
    jiri_script = """#!/bin/bash
if [ "$1" = "worktree" ]; then
    if [ "$2" = "add" ]; then
        target_path="$3"
        mkdir -p "$target_path"
        # Create an outdir so list shows something nice
        mkdir -p "$target_path/out/default"
        echo 'build_info_product = "fuchsia"' > "$target_path/out/default/args.gn"
        echo 'build_info_board = "x64"' >> "$target_path/out/default/args.gn"
        
        # We also need a dummy git repo so list/lease can run git commands inside it
        git init -q "$target_path"
        git -C "$target_path" config user.name "Test"
        git -C "$target_path" config user.email "test@test.com"
        touch "$target_path/dummy"
        git -C "$target_path" add dummy
        git -C "$target_path" commit -m "init" -q
        
        # Append to registry
        echo "$(realpath "$target_path")" >> "$(dirname "$0")/../worktrees_registry"
        exit 0
    elif [ "$2" = "remove" ]; then
        target_path="$3"
        if [ "$target_path" = "-f" ] || [ "$target_path" = "-force" ]; then
            target_path="$4"
        fi
        resolved_path="$(realpath "$target_path")"
        if [ -f "$(dirname "$0")/../worktrees_registry" ]; then
            # Filter out the removed path
            grep -v "^$resolved_path$" "$(dirname "$0")/../worktrees_registry" > "$(dirname "$0")/../worktrees_registry.tmp" || true
            mv "$(dirname "$0")/../worktrees_registry.tmp" "$(dirname "$0")/../worktrees_registry"
        fi
        rm -rf "$target_path"
        exit 0
    elif [ "$2" = "clean" ]; then
        exit 0
    fi
fi
exit 0
"""
    with open(mock_jiri_path, "w") as f:
        f.write(jiri_script)
    os.chmod(mock_jiri_path, 0o755)
            
    return test_dir, fuchsia_dir

def run_cmd(bin_path, fuchsia_dir, args):
    env = os.environ.copy()
    env["FUCHSIA_DIR"] = fuchsia_dir
    env["FORCE_COLOR"] = "1"
    res = subprocess.run([bin_path] + args, cwd=fuchsia_dir, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    return res.stdout

def generate_frames(bin_path):
    term = Terminal()
    frames = []

    def append_frame(show_cursor=True, duration=100):
        frames.append((term.draw(show_cursor), duration))

    def type_command(cmd):
        prompt = "\x1b[32m❯\x1b[0m "
        term.add_line(prompt)
        append_frame(show_cursor=True, duration=300)
        
        for i in range(len(cmd) + 1):
            typed = cmd[:i]
            term.update_last_line(prompt + typed)
            append_frame(show_cursor=True, duration=80)
        
        for _ in range(2):
            append_frame(show_cursor=False, duration=250)
            append_frame(show_cursor=True, duration=250)

    # Set up mock workspace for execution
    test_dir, fuchsia_dir = setup_mock_workspace()
    try:
        # 1. fx-worktree list (Initially empty)
        type_command("fx-worktree list")
        output = run_cmd(bin_path, fuchsia_dir, ["list"])
        for line in output.splitlines():
            line = line.replace(fuchsia_dir, "/usr/local/google/home/username/fuchsia")
            term.add_line(line)
        append_frame(show_cursor=True, duration=2000)

        # 2. fx-worktree add worktree1
        type_command("fx-worktree add worktree1")
        output = run_cmd(bin_path, fuchsia_dir, ["add", "worktree1"])
        for line in output.splitlines():
            line = line.replace(fuchsia_dir, "/usr/local/google/home/username/fuchsia")
            term.add_line(line)
        append_frame(show_cursor=True, duration=2500)

        # 3. fx-worktree list (Shows worktree1 as Free)
        type_command("fx-worktree list")
        output = run_cmd(bin_path, fuchsia_dir, ["list"])
        for line in output.splitlines():
            line = line.replace(fuchsia_dir, "/usr/local/google/home/username/fuchsia")
            term.add_line(line)
        append_frame(show_cursor=True, duration=2500)

        # 4. fx-worktree lease worktree1
        type_command("fx-worktree lease worktree1")
        output = run_cmd(bin_path, fuchsia_dir, ["lease", "worktree1"])
        for line in output.splitlines():
            line = line.replace(fuchsia_dir, "/usr/local/google/home/username/fuchsia")
            term.add_line(line)
        append_frame(show_cursor=True, duration=4000)

        # 5. fx-worktree list (Shows worktree1 as In Use)
        type_command("fx-worktree list")
        output = run_cmd(bin_path, fuchsia_dir, ["list"])
        for line in output.splitlines():
            line = line.replace(fuchsia_dir, "/usr/local/google/home/username/fuchsia")
            term.add_line(line)
        append_frame(show_cursor=True, duration=2500)

        # 6. fx-worktree release worktree1
        type_command("fx-worktree release worktree1")
        output = run_cmd(bin_path, fuchsia_dir, ["release", "worktree1"])
        for line in output.splitlines():
            line = line.replace(fuchsia_dir, "/usr/local/google/home/username/fuchsia")
            term.add_line(line)
        append_frame(show_cursor=True, duration=2500)

        # 7. fx-worktree remove worktree1
        type_command("fx-worktree remove worktree1")
        output = run_cmd(bin_path, fuchsia_dir, ["remove", "worktree1"])
        for line in output.splitlines():
            line = line.replace(fuchsia_dir, "/usr/local/google/home/username/fuchsia")
            term.add_line(line)
        append_frame(show_cursor=True, duration=2500)

        # 8. fx-worktree list (Empty again)
        type_command("fx-worktree list")
        output = run_cmd(bin_path, fuchsia_dir, ["list"])
        for line in output.splitlines():
            line = line.replace(fuchsia_dir, "/usr/local/google/home/username/fuchsia")
            term.add_line(line)
        append_frame(show_cursor=True, duration=2500)

        # Final blink
        for _ in range(2):
            term.add_line("          ") # Clear line
            term.update_last_line("\x1b[32m❯\x1b[0m ")
            append_frame(show_cursor=True, duration=250)
            term.update_last_line("\x1b[32m❯\x1b[0m")
            append_frame(show_cursor=False, duration=250)

    finally:
        shutil.rmtree(test_dir)

    return frames

if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: python3 generate_demo.py <output_path>")
        sys.exit(1)
    output_path = sys.argv[1]
    
    # Compile fx-worktree in release mode first to ensure we use the latest binary
    print("Compiling fx-worktree in release mode...")
    subprocess.run(["cargo", "build", "--release"], check=True)
    bin_path = os.path.abspath("target/release/fx-worktree")

    os.makedirs(os.path.dirname(output_path), exist_ok=True)
    
    print("Generating frames...")
    frames_data = generate_frames(bin_path)
    
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
