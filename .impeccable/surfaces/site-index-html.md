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

Features (2026-11-01): a bento of eight tiles in the page's theme, so the theme switcher recolors them. By priority: Split view and Layer previews get the big tiles, Plugins in the rail and Development plugins the halves, and Tabs, Discord, Copy as code and Color contrast the small ones. Each tile is a drawing and one line; the plus opens a sheet with the drawing building up, the full details and the settings key. Tried and dropped: a theme per feature (fought the switcher) and a pinned scroll-through window (too much scrolling)

Themes (2026-11-01): the words on the left, a fan deck on the right opening rightward, Figma's own in the middle. Each blade is its name and hue along the top, its accent ramp and mesh, riveted at the left. Pointing at a blade slides it out, clicking it retints the page with a short crossfade. A Light and Dark switch sits under the words. Below 900px wide the fan becomes the grid of swatches, each with light and dark orbs. A circle from the pointer and a wipe were both dropped. The upward fan with names up the edges was dropped as hard to read

Hero (2026-11-01): the Figma canvas that was the sign-off, now at the top: the Ligma lockup selected as a layer, "Same Figma. Lighter." and the one-liner under it, the downloads drawn as frames with a "You" cursor, and the reel of what's inside along its foot. Reel chips for features with a tile open that tile's sheet; the rest scroll to their section. The reel never pauses

Footer (2026-11-01): a centered stack, "Figma, without the weight.", the download and Star on GitHub, then a row of links

Highlighted controls on the accent, on the page and in the drawings, carry white text and icons

AI image tools (2026-11-01): pinned for 600svh (960 felt too slow), and the label under the picture follows the tool and its setting

Drawings (2026-11-01): drawn from Figma's UI3 as it is. The left sidebar is Figma's: the logo, the file, assets, add and resources buttons, the plugins button and the development plugin, then the plugins; the panel has the file name with Team project and plan, Pages, then Layers. Layer previews nest layers in their frames, with chevrons, type icons and a grey thumbnail per layer. The Actions panel has All, Assets and Plugins & widgets, an outlined Development filter, and rows with a Development tag and the manifest's path; the toolbar sits under it. Toolbar tools carry their dropdown chevrons. Cursors are Figma's rounded arrow with a square-cornered name tag. Call it the sidebar, never the rail

Order (2026-11-01): hero, themes, features, AI image tools, compare, install. Themes and the image tools swapped places

Fidelity (2026-11-01): every drawing is drawn at Figma's and Ligma's real size in px (11px Inter, 32px rows, 241px panels, 48px toolbar, 13px popover radius, dark menus and tooltips in either mode, the tab bar and preview card from dist/index.html and dist/preview.html, the contrast row from contrast.rs) and scaled to its frame as a screenshot crop (`.shot`, --w). Tiles sit on Figma's canvas grey, not a blue field, with a thin frame, no hover lift and a plain Details link in place of the round plus. Dropped as reading AI-made: em-scaled chunky mocks, grey placeholder bars, saturated colour fields behind every drawing

Features (2026-11-01, replaces the bento): an exploded Ligma window. Eight planes stacked in 3D, bottom to top: the split canvas, Figma's panels with layer previews, the left sidebar's plugins, the tab bar with its preview, then the Actions panel, the right-click menus, the color picker and the Discord card. Each plane is outlined in the accent and labelled beside the stack with its feature, one line and Details (the same sheet). Pinned for 340svh, it comes together into one flat window by three quarters of the way; the popovers fade as they land, so the window ends clean. Where it does not pin (narrow, short or reduced motion) the tiles show as a plain list. The user turned down a settings window, editorial rows, a spec list, a playable mock, a browser-vs-Ligma wipe and an annotated teardown

Features (2026-11-01, replaces the exploded window, which the user disliked): a Figma canvas, full bleed, dotted, panned by dragging (vertical wheel still scrolls the page; sideways wheel or Shift pans; pinch or Ctrl+wheel zooms). Each feature is a named frame at its real size with a Ligma comment pinned beside it carrying the title, a line, a point or two and the setting key. A floating Layers panel lists the frames and glides to one; the toolbar shows the hand tool on. On phones the panel becomes a row of frame names. The detail sheet and the bento are gone; the reel's feature chips glide to their frame

Canvas (2026-11-02): up and down over the canvas keep the page's momentum scrolling (Lenis is held off only for sideways gestures). Ctrl and the wheel or a pinch eases the zoom toward its goal about the pointer, about 10% a wheel notch. Each frame shows its own design in its own colors: a plant shop's app, post and story in split view, a coffee roaster's landing page under layer previews, an icon set beside the sidebar plugins, a pricing card under the copy menus. The themes editor keeps the poster. The smaller features sit under "And the smaller things"

Themes (2026-11-02, replaces the fan deck): multiplayer cursors. Nine cursors, one per theme in its accent with its name in the tag, wander over the editor mock like collaborators. Clicking one spreads its theme from that cursor (a View Transitions circle) and its tag becomes "You · <name>". The words and the Light and Dark switch sit above the editor, and a row of named mesh chips under it is the keyboard, screen reader and phone control. The user turned down a variables-modes table, Ligma's settings window and drag-to-paint

Themes (2026-11-02, replaces the cursors, which felt silly): Figma's Local variables window overlapping the foot of the editor mock. A "Ligma themes" collection, a mode column per theme with eight color variables (accent/default, accent/pressed, selection, bg/panel, bg/canvas, border, text/primary, text/secondary) taken from themes.js for the page's mode. Light and Dark are its groups in the sidebar. Clicking a column or a group retints the page with the crossfade. On phones the groups sit above the table, which scrolls sideways with the names pinned

Themes (2026-11-02, replaces the Variables window, too much for casual Figma users): the nine names set large in the display face beside the words, each in its own accent (700 on light, 400 on dark). Pointing at or tabbing to a name tries it on the whole page with a short ease, the others fade, moving away goes back; clicking keeps it with the crossfade and underlines it. Touch goes straight to a pick. The Light and Dark switch and the editor below stay

Themes (2026-11-02, chosen from three still mockups after the big names read as generic): Ligma's own Settings window, open at Appearance > Themes as drawn in dist/settings.html, floating over the editor mock. Each theme card has its light and dark orb; pointing at or tabbing to an orb tries that theme and mode on the page, clicking keeps it, with the ring and the sun or moon badge as in the app. The orbs carry the mode, so there is no separate Light and Dark switch. On narrow screens the window sits under the editor without its sidebar. Rejected on the way: the fan, multiplayer cursors, a Variables table, the big names, a contact sheet, themes as tabs

Themes (2026-11-02, the user's own idea, replacing the Settings window): Figma Draw's pencils. The editor mock's toolbar becomes a tray of nine pencils, one per theme, drawn after Draw's pencil (round body, sharpened wood, lead) with the body and lead in the theme's accent for the page's mode. They stand half sunk in the tray; pointing lifts one a little with a dark tooltip of its name, picking pokes it out with a springy ease and it stays out while the page takes its theme. The Light and Dark switch sits beside the words. The poster moved up to leave the pencils room

Themes (2026-11-02, refined): the pencils stand in a row behind the editor window, not in its toolbar, their tips and a little of their colored bodies peeking over the top edge. Pointing lifts one 8px with its name in a dark tooltip, picking pokes it up and it stays. The editor keeps its own toolbar and the poster its place

Pencils (2026-11-02): 50px wide (32px on phones), almost touching, peeking 1.9 widths over the window. Figma's own pencil is Draw's neutral grey with a graphite lead, so blue is never mistaken for it
