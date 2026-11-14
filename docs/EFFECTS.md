# Effects library

The Effects button in Figma Design's toolbar, after Actions, lists effects Ligma makes right in the file (`src-tauri/src/effects.rs`). This is how they're built, and what's planned.

## How effects work

- **No plugins.** Ligma's page script runs in Figma's page, where Figma's console plugin API, `window.figma`, works on the open file. Effects call it directly, so there's no plugin window, no plugin sandbox, no network and no paywall. The API exists only in files that can be edited, so the button only shows there.
- **Live in the file.** What an effect makes is real layers on the canvas from the first change, updated once a frame as you drag a field. Cancel removes them. Apply calls `figma.commitUndo()` once, so the whole result is one undo step.
- **Figma's own controls.** Panels and menus are drawn from Figma's CSS variables (`--color-bg`, `--color-bg-secondary`, `--color-border`, `--color-bg-menu`, `--elevation-500`, `--radius-large` and so on), which Figma sets on the page's body for the light or dark theme. They use its type (Inter, 11px), its dropdowns, segmented controls and menus, and its brand blue on sliders and the main button, so they sit with Figma's own panels rather than looking like a plugin's.
- **Adding one.** Each effect is an entry in `EFFECTS` (`id`, `name`, `about`, `icon`, `open`). `open(figma)` gets the plugin API and the selection, and builds its panel from the shared controls: `select`, `segments`, sections and the footer, as Repeater's does with its labeled sliders.
- **Editable results.** An effect can keep what it made in a group tagged with shared plugin data (`ligma` namespace), with its settings, so selecting the group and opening the effect again changes it instead of starting over.

## Repeater (built)

Repeats the selected layer, live. Its panel has a preview of the whole result up top, drawn from a picture of the layer, so it shows even when the copies are off screen; preset chips; Repeat as; labeled sliders, each with its value beside it, where distances and turns ease out from zero so small values get most of the slider, and End size has 100% in the middle; values can be typed in past the slider's ends, dragged left and right as Figma's fields are, or stepped with the arrow keys (Shift by 10); Reset; and Place, which says how many copies it places. The preview is grouped from the start, so the layers panel shows one group, and Cancel puts the original back where it was (not in auto layout, where a group would join the flow):

- **Trail:** each copy a step right and down from the last, turned by a set angle, and curving: the step's direction turns each time, so trails bend into arcs, and each step is as much smaller or larger as the copies are, so shrinking copies spiral inward.
- **Radial:** around a ring through the original, over a sweep of up to 360°, turning the copies to follow the ring or not.
- **Grid:** rows and columns, with gaps.
- **To the last copy:** size, opacity and a blend toward a color change from the first copy to the last, evenly or easing in, out or both.
- **Presets:** Trail, Fade out, Spiral, Arc, Ring, Fan, Grid and Echo.
- **Result:** copies stacked under or over the original. With Keep editable (on by default) the original and its copies are grouped as a Repeater: select it and open Repeater to change it, and the copies are made again from the original as it is then, so they follow changes to it. Without it, the copies are plain layers.
- **Limits:** up to 400 copies. Copies in auto layout are placed absolutely. Sections can't be repeated.

## Disintegrate (built)

Breaks the selected layer into pieces and moves them, live, as After Effects' Shatter does, with Snap for the Thanos-snap dust look:

- **Shape:** Pattern (Glass, Squares, Bricks or Triangles) and how many Pieces, about. Glass is Voronoi cells around a seed jittered in each square of a grid, each cut by the lines halfway to the seeds around it, so every piece is a polygon with straight edges. Shuffle changes the pattern.
- **Forces:** one or two, as AE's Force 1 and Force 2, each with a Position, a Radius and a Strength. Only the pieces within a force's radius break, so the damage can stay close to the subject. Each force shows in the preview as a dot and a dashed ring: drag the dot to move it, or the ring to change its radius.
- **Time:** how far along it is, 0 whole to 100.
- **Shatter's physics:** pieces are thrown out from each force, harder near its middle, by its strength; Rotation speed turns them, Randomness varies each one's direction and speed, Viscosity slows them, and Gravity pulls them in its direction.
- **Snap's dust:** a ragged front, Raggedness and Edge softness, spreads out from each force as Time goes, as far as its radius; the pieces it passes leave, slow at first then faster, with the wind (Wind direction), as far as the force's strength takes them, scattering, shrinking, fading late so a trail stays near what's left, and lightening.
- **Drawing:** the layer is exported as a picture (up to 2× and 4,096 pixels across); the moving pieces are cut out of it and each drawn where it goes with the canvas, turned and sized about its center and clipped to its polygon, so edges stay smooth and sharp. The preview draws at once; the image in the file follows a moment after, and Disintegrate makes it at full size.
- **Presets:** Drift and Dust (Snap); Glass, Crack, Explode and Tiles (Shatter). Picking a mode starts from its first preset, keeping the forces.
- **Result:** an image of what's drawn, over the original. With Keep editable (on by default) the original is kept under it, hidden, in a Disintegrate group: select it and open Disintegrate to change it, made again from the original. Without it, only the image is left.

## Planned

### Bitcrush shader: feasible, best as a native shader

Pixelate, posterize and dither a layer.

- Figma now has shader effects of its own (Effects > Shader effects), which run live on the canvas, non-destructively. If they can be made through the plugin API or Figma's effect data, Bitcrush should be one: pixel size, bit depth, dither, color mode and mix, all live and editable later in the right panel.
- Without that, it works destructively: export the layer, process it on a canvas (downsample, quantize to the bit depth or a palette, ordered or error-diffusion dither, optional RGB split), and put the result back as an image fill, previewed live at screen size and made at full size on Apply.
- Next step: check what the plugin API exposes for shader effects before building it.
