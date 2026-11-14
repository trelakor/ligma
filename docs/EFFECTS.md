# Effects library

The Effects button in Figma Design's toolbar, after Actions, lists effects Ligma makes right in the file (`src-tauri/src/effects.rs`). This is how they're built, and what's planned.

## How effects work

- **No plugins.** Ligma's page script runs in Figma's page, where Figma's console plugin API, `window.figma`, works on the open file. Effects call it directly, so there's no plugin window, no plugin sandbox, no network and no paywall. The API exists only in files that can be edited, so the button only shows there.
- **Live in the file.** What an effect makes is real layers on the canvas from the first change, updated once a frame as you drag a field. Cancel removes them. Apply calls `figma.commitUndo()` once, so the whole result is one undo step.
- **Figma's own controls.** Panels and menus are drawn from Figma's CSS variables (`--color-bg`, `--color-bg-secondary`, `--color-border`, `--color-bg-menu`, `--elevation-500`, `--radius-large` and so on), which Figma sets on the page's body for the light or dark theme. They use its type (Inter, 11px), its dropdowns, segmented controls and menus, and its brand blue on sliders and the main button, so they sit with Figma's own panels rather than looking like a plugin's.
- **Adding one.** Each effect is an entry in `EFFECTS` (`id`, `name`, `about`, `icon`, `open`). `open(figma)` gets the plugin API and the selection, and builds its panel from the shared controls: `select`, `segments`, sections and the footer, as Repeater's does with its labeled sliders.
- **Editable results.** An effect can keep what it made in a group tagged with shared plugin data (`ligma` namespace), with its settings, so selecting the group and opening the effect again changes it instead of starting over.

## Repeater (built)

Repeats the selected layer, live. Its panel has a preview of the whole result up top, drawn from a picture of the layer, so it shows even when the copies are off screen; preset chips; Repeat as; labeled sliders, each with its value beside it, which can be typed in past the slider's ends (arrow keys step it, Shift by 10); Reset; and Place, which says how many copies it places:

- **Trail:** each copy a step right and down from the last, turned by a set angle, and curving: the step's direction turns each time, so trails bend into arcs, and each step is as much smaller or larger as the copies are, so shrinking copies spiral inward.
- **Radial:** around a ring through the original, over a sweep of up to 360°, turning the copies to follow the ring or not.
- **Grid:** rows and columns, with gaps.
- **To the last copy:** size, opacity and a blend toward a color change from the first copy to the last, evenly or easing in, out or both.
- **Presets:** Trail, Fade out, Spiral, Arc, Ring, Fan, Grid and Echo.
- **Result:** copies stacked under or over the original. With Keep editable (on by default) the original and its copies are grouped as a Repeater: select it and open Repeater to change it, and the copies are made again from the original as it is then, so they follow changes to it. Without it, the copies are plain layers.
- **Limits:** up to 400 copies. Copies in auto layout are placed absolutely. Sections can't be repeated.

## Planned

### Shatter: feasible

Break a layer into shards that fly apart.

- **Vectors and shapes:** flatten the layer (`figma.flatten`), build Voronoi cells over its bounds from seeded random points (denser near a chosen impact point), and intersect each cell with a copy of the shape (`figma.intersect`), giving real vector shards that keep the fill.
- **Images and anything else:** export the layer (`exportAsync` PNG), and for each cell a copy of the image fill masked by the cell polygon, or each shard's pixels cut out on a canvas and placed back with `figma.createImage`.
- **Live controls:** shard count, impact point (dragged on the canvas), spread, rotation, scatter falloff and gap. Shards move from the impact point outward, each turned a little.
- **Cost:** boolean operations are the slow part. Previewing with simple polygon fills and doing the real cut on Apply keeps it live.

### Thanos snap: feasible, as frames

Dissolve a layer into dust drifting away.

- Export the layer at its size, sample its pixels into particles (a few thousand, larger where it's sparse), and place them as small rectangles or a handful of images, drifting in one direction with noise, fading and shrinking, more of the layer gone toward one side.
- Figma has no timeline, so the animation is a set of frames (the steps from whole to gone), ready for Smart Animate in a prototype or exporting as a sequence. A single frame works as a still.
- **Live controls:** progress (scrubs the dissolve), direction, particle size, turbulence and seed.
- **Cost:** thousands of layers are heavy. Grouping particles into a few images per step, drawn on a canvas, keeps the file light.

### Bitcrush shader: feasible, best as a native shader

Pixelate, posterize and dither a layer.

- Figma now has shader effects of its own (Effects > Shader effects), which run live on the canvas, non-destructively. If they can be made through the plugin API or Figma's effect data, Bitcrush should be one: pixel size, bit depth, dither, color mode and mix, all live and editable later in the right panel.
- Without that, it works destructively: export the layer, process it on a canvas (downsample, quantize to the bit depth or a palette, ordered or error-diffusion dither, optional RGB split), and put the result back as an image fill, previewed live at screen size and made at full size on Apply.
- Next step: check what the plugin API exposes for shader effects before building it.
