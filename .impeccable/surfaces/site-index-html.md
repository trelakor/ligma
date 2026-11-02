---
version: 1
slug: "site-index-html"
primary_target: "site/index.html"
related_targets: []
---

Scope: the GitHub Pages landing page (site/index.html). Visitor mode: Persuade. The visitor is a designer who lives in Figma. The job is to show they can have the same Figma at a fraction of the weight, plus extras, and get them to download. Lane pinned by the user: product-launch editorial (big type, a drawn illustration per feature). The whole site retints when a theme orb is clicked, and the choice isn't remembered.

## Direction contract

THESIS: Ligma's launch page reads like a swatch book. Each idea is a colour chip with a field and a label strip, and the nine themes are the fan deck. It refuses the centred hero over a row of equal icon cards.

OWN-WORLD: The ground is Figma's grey ramp, tinted by the active theme through dist/themes.js. Chips are rectangles in one aspect, with a full colour field in the theme's accent-500 inked in accent-1000, above a label strip. Funnel Display sets the headlines; Inter sets the body and labels with tabular figures. Icons are drawn in the app's 16px stroke-1.2 grammar. Everything sits on an 8px module.

STORY: It's the same Figma, at 16 MB instead of 305. Here's what you also get, shown working. Try every theme on this page. Then download for your OS.

FIRST VIEWPORT: A huge headline on the left over two lines, with the OS-detected download button and the version beneath it. Below that, an area-true size chart runs full width: a grey 305 MB field for Figma desktop holds a vivid 16 MB Ligma chip in its corner, scaled by area.

FORM: Swatch book / fan deck, #4 of the ordered list. seed:f28eea57c344b988. Raises: from brick build instructions, a strict module grid and illustrations that teach without words; from the VU meter bridge, identical chips repeated as a ruler (the theme strip).

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance

Scroll story (added 2026-11-01): momentum scrolling (site/vendor/lenis.min.js, off for reduced motion and on touch). On screens at least 900 × 700 the hero and the AI image tools pin while they play out: the grey 305 MB Figma desktop block shrinks by area to the 16 MB corner and turns into the blue Ligma chip; the image tools step through Select object, Remove background and Vectorize on one photo under Figma's image bar. Feature drawings build up as they come into view. Reduced motion shows every scene finished.

Signature interaction: clicking a theme orb retints the whole page and switches light or dark. The new palette spreads as a circle from the orb (View Transitions), and a large Figma editor mock in the themes section shows the theme on Figma's interface while the canvas design stays untouched.

Features (2026-11-01): one Figma window, all in the page's theme so the theme switcher recolors it. On large screens it pins, and the scroll selects each of the eight features as a layer in the left panel; the canvas shows its drawing in a selected frame and the properties panel its copy and settings key. Clicking a layer scrolls to it. Elsewhere the features are a list of frames with their copy. Per-feature themes were tried and dropped: they fought the theme switcher
