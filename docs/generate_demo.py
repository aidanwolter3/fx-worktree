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

def setup_mock_workspace():
    test_dir = tempfile.mkdtemp(prefix="fx-worktree-demo-")
    fuchsia_dir = os.path.join(test_dir, "fuchsia")
    os.makedirs(fuchsia_dir)
    
    # Create .jiri_root
    jiri_root = os.path.join(fuchsia_dir, ".jiri_root")
    os.makedirs(jiri_root)
    os.makedirs(os.path.join(jiri_root, "leases"))
    os.makedirs(os.path.join(jiri_root, "worktrees"))
    
    # Create worktrees
    wt_ids = ["fuchsia.x64-37954053"]
    for wt_id in wt_ids:
        wt_path = os.path.join(jiri_root, "worktrees", wt_id)
        os.makedirs(wt_path)
        
        # Init git repo
        subprocess.run(["git", "init", "-q", wt_path])
        subprocess.run(["git", "-C", wt_path, "config", "user.name", "Test"])
        subprocess.run(["git", "-C", wt_path, "config", "user.email", "test@test.com"])
        with open(os.path.join(wt_path, "dummy"), "w") as f:
            f.write("dummy")
        subprocess.run(["git", "-C", wt_path, "add", "dummy"])
        subprocess.run(["git", "-C", wt_path, "commit", "-m", "init", "-q"])
        
        # Create args.gn
        out_dir = os.path.join(wt_path, "out", "default")
        os.makedirs(out_dir)
        with open(os.path.join(out_dir, "args.gn"), "w") as f:
            f.write('build_info_product = "fuchsia"\n')
            f.write('build_info_board = "x64"\n')
            
    # Create mock jiri script in parent .jiri_root/bin/jiri
    mock_jiri_dir = os.path.join(jiri_root, "bin")
    os.makedirs(mock_jiri_dir)
    mock_jiri_path = os.path.join(mock_jiri_dir, "jiri")
    with open(mock_jiri_path, "w") as f:
        f.write("#!/bin/bash\n")
        f.write("if [ \"$1\" = \"worktree\" ] && [ \"$2\" = \"clean\" ]; then\n")
        f.write("    exit 0\n")
        f.write("fi\n")
        f.write("exit 0\n")
    os.chmod(mock_jiri_path, 0o755)
            
    return test_dir, fuchsia_dir

def run_cmd(bin_path, fuchsia_dir, fx_worktree_root, args):
    env = os.environ.copy()
    env["FUCHSIA_DIR"] = fuchsia_dir
    env["FX_WORKTREE_ROOT"] = fx_worktree_root
    res = subprocess.run([bin_path] + args, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    return res.stdout

def colorize_line(line, fuchsia_dir):
    # 1. Replace paths
    line = line.replace(fuchsia_dir, "/usr/local/google/home/username/fuchsia")
    
    # 2. Check line type and apply exclusive colorization
    if line.startswith("✔ "):
        line = "[[g]]✔[[w]]" + line[1:]
        # Colorize ID in this line
        line = re.sub(r'(fuchsia\.x64-[a-f0-9]+)', r'[[b]]\1[[w]]', line)
        return line
    
    if "Worktree ID" in line or "Agent ID" in line or "Config" in line or "Path" in line:
        match = re.match(r'(\s*[a-zA-Z ]+\s*:\s*)(.*)', line)
        if match:
            val = match.group(2)
            return match.group(1) + f"[[b]]{val}[[w]]"
        return line

    if "fx-worktree cd" in line:
        match = re.match(r'(\s*\$\s*fx-worktree\s*cd\s*)([a-zA-Z0-9.-]+)?(.*)', line)
        if match:
            cmd = match.group(1)
            id = match.group(2) or ""
            rest = match.group(3) or ""
            if id:
                return f"{cmd}[[b]]{id}[[w]]{rest}"
            return line
        return line

    if "CONFIG " in line and "WORKTREE ID" in line:
        return line

    if line.startswith("Resetting worktree "):
        line = re.sub(r'(fuchsia\.x64-[a-f0-9]+)', r'[[b]]\1[[w]]', line)
        return line

    if "Free" in line or "In Use" in line:
        line = line.replace("Free", "[[g]]Free[[w]]")
        line = re.sub(r'In Use \((agent-[a-f0-9]+)\)', r'[[y]]In Use (\1)[[w]]', line)
        match = re.match(r'^([a-zA-Z0-9.-]+)(\s+)([a-zA-Z0-9.-]+)(\s+)(.*)', line)
        if match:
            cfg = match.group(1)
            space1 = match.group(2)
            id = match.group(3)
            space2 = match.group(4)
            rest = match.group(5)
            return f"[[b]]{cfg}[[w]]{space1}[[b]]{id}[[w]]{space2}{rest}"
        return line

    return line

def generate_frames(bin_path):
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

    # Set up mock workspace for execution
    test_dir, fuchsia_dir = setup_mock_workspace()
    fx_worktree_root = os.path.join(test_dir, ".fx_worktree_root")
    os.makedirs(fx_worktree_root)

    try:
        # 1. fx-worktree list
        type_command("fx-worktree list")
        output = run_cmd(bin_path, fuchsia_dir, fx_worktree_root, ["list"])
        for line in output.splitlines():
            term.add_line(colorize_line(line, fuchsia_dir))
        append_frame(show_cursor=True, duration=2000)

        # 2. fx-worktree lease fuchsia.x64
        type_command("fx-worktree lease fuchsia.x64")
        output = run_cmd(bin_path, fuchsia_dir, fx_worktree_root, ["lease", "fuchsia.x64"])
        leased_id = None
        for line in output.splitlines():
            term.add_line(colorize_line(line, fuchsia_dir))
            if "Worktree ID" in line:
                match = re.search(r'fuchsia\.x64-[a-f0-9]+', line)
                if match:
                    leased_id = match.group(0)
        append_frame(show_cursor=True, duration=4000)

        # 3. fx-worktree list again
        type_command("fx-worktree list")
        output = run_cmd(bin_path, fuchsia_dir, fx_worktree_root, ["list"])
        for line in output.splitlines():
            term.add_line(colorize_line(line, fuchsia_dir))
        append_frame(show_cursor=True, duration=2500)

        # 4. fx-worktree release <id>
        if leased_id:
            type_command(f"fx-worktree release {leased_id}")
            output = run_cmd(bin_path, fuchsia_dir, fx_worktree_root, ["release", leased_id])
            for line in output.splitlines():
                term.add_line(colorize_line(line, fuchsia_dir))
            append_frame(show_cursor=True, duration=2500)

        # Final blink
        for _ in range(2):
            term.add_line("          ") # Clear line
            term.update_last_line("[[g]]❯[[w]] ")
            append_frame(show_cursor=True, duration=250)
            term.update_last_line("[[g]]❯[[w]]")
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
    bin_path = "target/release/fx-worktree"

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
