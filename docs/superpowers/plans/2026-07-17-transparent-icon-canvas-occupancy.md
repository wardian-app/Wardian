# Transparent Icon Canvas Occupancy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Wardian's transparent app mark use more of its canvas without changing the lantern artwork or clipping small assets.

**Architecture:** Generate an enlarged 512px transparent master from `public/icon-transparent.png`. Resample existing transparent PNG and ICO frames individually to preserve each platform's alpha treatment, and use the repository's Tauri CLI only for the ICNS container. Keep the existing 16px and 24px PNG/ICO frames because their current one-pixel edge buffer leaves no space for a larger mark.

**Tech Stack:** PNG/ICO/ICNS assets, Python Pillow, `@tauri-apps/cli` 2.11.2.

## Global Constraints

- Modify only `src-tauri/icons/transparent/`; leave the opaque and white icon families unchanged.
- Do not redraw, crop, recolor, or otherwise modify the lantern artwork.
- Use a maximum 7 percent scale; retain existing 16px and 24px frames to preserve a one-pixel edge buffer.
- Keep every existing transparent `bundle.icon` entry in `src-tauri/tauri.conf.json`.
- Do not introduce runtime dependencies or application code.

**Tooling finding:** The installed Tauri 2.11.2 generator writes opaque 32px PNGs from this transparent master. Do not copy its PNG, ICO, Android, or iOS outputs; use it only to generate `icon.icns`, whose alpha is preserved.

---

## File Structure

- Modify: `src-tauri/icons/transparent/**` — regenerated transparent raster and container assets.
- Modify: `docs/superpowers/specs/2026-07-17-transparent-icon-canvas-occupancy-design.md` — small-raster safety rule.
- Create locally, do not commit: `e2e/screenshots/icon-canvas-occupancy/<timestamp>/windows-taskbar-icon.png` — taskbar evidence.

### Task 1: Generate the safe transparent asset family

**Files:**

- Modify: `src-tauri/icons/transparent/**`
- Modify: `docs/superpowers/specs/2026-07-17-transparent-icon-canvas-occupancy-design.md`

**Interfaces:**

- Consumes: unchanged `public/icon-transparent.png` (512px transparent master).
- Produces: complete `src-tauri/icons/transparent/` output compatible with the current Tauri bundle configuration.

- [ ] **Step 1: Verify the baseline proves the smallest frames cannot grow safely**

Run:

```powershell
@'
from pathlib import Path
from PIL import Image

for name in ('16x16.png', '24x24.png', '32x32.png'):
    image = Image.open(Path('src-tauri/icons/transparent') / name).convert('RGBA')
    print(name, image.size, image.getchannel('A').getbbox())
'@ | python -
```

Expected: 16px and 24px frames have a one-pixel alpha edge on every side; 32px has a two-pixel edge.

- [ ] **Step 2: Run the desired 32px occupancy assertion before changing assets**

Run:

```powershell
@'
from PIL import Image

image = Image.open('src-tauri/icons/transparent/32x32.png').convert('RGBA')
left, top, right, bottom = image.getchannel('A').getbbox()
assert right - left >= 30 and bottom - top >= 30, (right - left, bottom - top)
'@ | python -
```

Expected: FAIL with the current 28px by 28px occupied bounds. This proves the asset update has a measurable target.

- [ ] **Step 3: Create the enlarged master and regenerate into an isolated directory**

Run:

```powershell
$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("wardian-icon-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tempRoot -ErrorAction Stop | Out-Null
$tempMaster = Join-Path $tempRoot 'icon.png'
Copy-Item -LiteralPath 'src-tauri/icons/transparent/icon.ico' -Destination (Join-Path $tempRoot 'original.ico') -Force
@'
import sys
from PIL import Image

source = Image.open('public/icon-transparent.png').convert('RGBA')
left, top, right, bottom = source.getchannel('A').getbbox()
art = source.crop((left, top, right, bottom))
scaled = art.resize((round(art.width * 1.07), round(art.height * 1.07)), Image.Resampling.LANCZOS)
result = Image.new('RGBA', source.size, (0, 0, 0, 0))
result.alpha_composite(scaled, ((source.width - scaled.width) // 2, (source.height - scaled.height) // 2))
result.save(sys.argv[1])
'@ | python - $tempMaster
npx tauri icon $tempMaster --output (Join-Path $tempRoot 'icons')
```

Expected: `$tempRoot/icons` contains a complete Tauri icon family derived from the enlarged, centered mark.

- [ ] **Step 4: Copy only the transparent generated outputs, then restore the two source-limited PNG files**

Run:

```powershell
Copy-Item -LiteralPath 'src-tauri/icons/transparent/16x16.png' -Destination (Join-Path $tempRoot '16x16.png') -Force
Copy-Item -LiteralPath 'src-tauri/icons/transparent/24x24.png' -Destination (Join-Path $tempRoot '24x24.png') -Force
Copy-Item -LiteralPath (Join-Path $tempRoot 'icons/*') -Destination 'src-tauri/icons/transparent' -Recurse -Force
Copy-Item -LiteralPath (Join-Path $tempRoot '16x16.png') -Destination 'src-tauri/icons/transparent/16x16.png' -Force
Copy-Item -LiteralPath (Join-Path $tempRoot '24x24.png') -Destination 'src-tauri/icons/transparent/24x24.png' -Force
```

Expected: every transparent platform asset is regenerated except the 16px and 24px PNG assets.

- [ ] **Step 5: Restore the matching 16px and 24px ICO frames**

The original ICO is saved in Step 2. Then run:

```powershell
@'
import sys
from PIL import Image

original = Image.open(sys.argv[1])
generated = Image.open('src-tauri/icons/transparent/icon.ico')
frames = {size: generated.ico.getimage(size).convert('RGBA') for size in generated.ico.sizes()}
for size in ((16, 16), (24, 24)):
    frames[size] = original.ico.getimage(size).convert('RGBA')
base = frames[(256, 256)]
base.save(
    'src-tauri/icons/transparent/icon.ico',
    format='ICO',
    sizes=sorted(frames),
    append_images=[frames[size] for size in sorted(frames) if size != (256, 256)],
)
'@ | python - (Join-Path $tempRoot 'original.ico')
```

Expected: the ICO keeps original 16px/24px frames and enlarged 32px-or-larger frames.

- [ ] **Step 6: Review and commit the asset change**

Run:

```powershell
git diff --stat
git diff -- src-tauri/tauri.conf.json
git status --short
git add src-tauri/icons/transparent docs/superpowers/specs/2026-07-17-transparent-icon-canvas-occupancy-design.md
git commit -m "fix(icons): enlarge transparent app mark"
```

Expected: no opaque, white, or Tauri configuration assets are staged.

### Task 2: Verify bounds, configuration, build, and Windows taskbar rendering

**Files:**

- Verify: `src-tauri/icons/transparent/**`
- Verify: `src-tauri/tauri.conf.json:40-46`
- Create locally: `e2e/screenshots/icon-canvas-occupancy/<timestamp>/windows-taskbar-icon.png`

**Interfaces:**

- Consumes: the regenerated transparent asset family from Task 1.
- Produces: alpha-bound verification output, build output, and a focused Windows taskbar screenshot.

- [ ] **Step 1: Programmatically prove safety margins and enlargement**

Run:

```powershell
@'
from pathlib import Path
from PIL import Image

def check(label, image):
    image = image.convert('RGBA')
    left, top, right, bottom = image.getchannel('A').getbbox()
    margins = (left, top, image.width-right, image.height-bottom)
    print(f'{label}: alpha={right-left}x{bottom-top}; margins={margins}')
    assert min(margins) >= 1, f'{label} lost its safety edge'

for path in sorted(Path('src-tauri/icons/transparent').rglob('*.png')):
    if '/ios/' not in path.as_posix():
        check(path.as_posix(), Image.open(path))
ico = Image.open('src-tauri/icons/transparent/icon.ico')
for size in sorted(ico.ico.sizes()):
    check(f'icon.ico:{size}', ico.ico.getimage(size))
'@ | python -
```

Expected: all non-iOS frames retain a nonzero edge buffer; the 16px/24px frames match their original dimensions and the 32px frame gains visual occupancy.

- [ ] **Step 2: Verify bundle configuration and compile the frontend**

Run:

```powershell
Get-Content src-tauri/tauri.conf.json | Select-String 'icons/transparent/'
npm run lint
npm run build
```

Expected: all five transparent bundle references remain present, and both commands pass.

- [ ] **Step 3: Build the debug Windows bundle and capture the changed taskbar state**

Run:

```powershell
npm run tauri -- build --debug
```

Launch the generated executable, then capture one screenshot of the running Wardian taskbar button beside neighboring icons at normal Windows taskbar scale. Save it under `e2e/screenshots/icon-canvas-occupancy/<timestamp>/windows-taskbar-icon.png`.

Expected: the full lantern appears visibly larger without distortion, clipping, or an opaque square.

- [ ] **Step 4: Confirm the handoff state**

Run:

```powershell
git status --short --branch
git log --oneline -2
```

Expected: the branch contains the design commit and the icon asset commit, with only the ignored local screenshot as a possible artifact.
