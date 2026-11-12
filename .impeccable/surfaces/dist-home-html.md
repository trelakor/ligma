---
version: 1
slug: "dist-home-html"
primary_target: "dist/home.html"
related_targets: []
---

Scope: Ligma's own home page, replacing Figma's file browser (figma.com/files) in the Home tab for daily jobs: open, find, create and organize files. Rare jobs (admin, billing, sharing permissions, Community) open Figma's own page. Visitor mode: Operate. The visitor is a designer who opens Home dozens of times a day to get back to a file. Stage one is a clickable prototype with realistic sample data (dist/home.html); live Figma data comes after.

Pain points named by the user: slow load and laggy UI, finding files, clutter and upsells, organizing files.

## Direction contract

THESIS: Home is your work history. Files you opened, edited or were mentioned in sit under Today, Yesterday, This week and on by month, and one keystroke opens a search that knows every file, project and action. It refuses Figma's tabbed Recents grid with promos, a plan switcher and a prompt box over your files.

OWN-WORLD: Figma UI3 as Ligma draws it: Inter 11/13px, 32px rows, Figma's grey ramp, the theme's accent for selection, focus and primary actions only, 16px stroke-1.2 icons, product glyphs in Design blue, FigJam purple and Slides orange. Each thumbnail is a real file preview on canvas grey, inside a thin frame.

STORY: Home opens in under a frame, already on what you touched today. Comments waiting for you are marked on the file. Older work is one click on the month rail. Anything else is Ctrl K and a few letters.

FIRST VIEWPORT: Ligma's tab bar, a 240px sidebar (account, Search, Recents, Starred, Drafts, Teams with projects, Trash), a header with the search field and the New split button, then day sections of 4-up file cards with name, project and the latest activity, and a 48px month rail on the right.

FORM: Work timeline, dealt lead of three (Command home's search is folded in as the Ctrl K palette, at the user's request). seed:5bca95d77e92eb79

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance

Signature move: the month rail. It is a scrubber for your history. Hovering shows how many files each month holds, clicking glides there, and it follows as you scroll.

Search (from Command home): a large field with scope tokens (in: project, type:, by: person), results grouped as Files, Projects and Actions, create-from-search ("New Design file named …"), recents as a dense list while the field is empty, and a footer of key hints. It opens from the header field, the sidebar or Ctrl K.

Organizing: select with click, Shift or Ctrl, or drag a selection box over empty space (Ctrl, Cmd or Shift adds to the selection; the list scrolls at the edges); right-click or the row menu for Open, Open in new tab, Star, Rename (inline), Move to…, Duplicate, Copy link and Move to trash, with undo. The keys match: Enter, S, R, M, Delete. Dragging a card onto a sidebar project moves it.

Inbox (2026-11-11, asked for by the user, then moved out of the page): a bell beside the account menu at the top of the sidebar, with an unread count, opens a 420px window under it. It lists notifications as Figma's bell does (comments, replies, mentions, file and team invites, edit requests), grouped by day, each with who, what, the comment quoted in two lines, the file's preview and inline Approve/Deny or Join/Decline. Filters: All, Unread, Mentions, Requests; Mark all as read with undo; arrows move, Enter opens the file at the comment and closes the window, U toggles read, Esc or a click outside closes it. Reading a comment notification clears it from the file's count. A full-page inbox in the sidebar was tried and dropped as too heavy. Live data: the bell feed Ligma already reads in notifications.rs

Teams (2026-11-11): the sidebar follows Figma's plans. A Starter or Professional team is its own plan and sits under "Teams" with its tier beside it; an Organization gets a header with its teams under it. All Drafts show together in one view

Round two (2026-11-11, after using it live): card tools have the star rightmost, lined up with the menu button. Teams can be pinned to the top (star on hover, or right-click), Ligma-side per account; every heading (Pinned teams, Teams, each organization) folds away; teams show their own pictures and open to projects, their Drafts and their Trash. File types use Figma's own icons and colors; New opens a menu styled after the More menu on Figma's home (40px rows, 24px icons): every product, then Project and Import. The inbox words a notification as Figma's bell does (who, then "what · where"), shows the file's picture or a plain icon, and toggles read and unread. No sync status in the sidebar: only the bar over the files when Figma can't be reached

Home view (2026-11-12, asked for by the user, after Google Drive's home): the page now opens on Home, not Recents, with Home first in the sidebar. Four sections, each folding away (remembered) and each file shown once, in the first section that has it: Starred (one row, in the user's own order, pick-up drag works here too, View all), Suggested projects (one row of project tiles, by the latest activity in them, right-click for Open in Figma), Suggested files (a line each: name, why, where, who's in it now; why is new comments, someone in it now, or new changes at least 6 h after the user last opened it, filled with older work Recents has scrolled past) and Recents (two rows, no timeline, View all). Home's cards are smaller (200px min) than the full lists (240px). The month rail stays with Recents. Figma's word "projects" is used for folders

Home, round two (2026-11-12): Starred cards are the biggest on the page (300px min); Suggested projects are tiles with a mosaic of the project's latest previews, its team, last change and file count (from Figma), one row; Suggested files are cards whose second line says why; Recents shows rows with a 64px preview, name, where and what happened, or cards, switched by a List/Grid toggle beside View all and on the Recents page (one setting for both, rows by default). Opening a team counts its projects and Drafts (ProjectFileCount) and loads its Trash from Figma (TrashedFilesPageView), so the team's Trash is Figma's own

Round three (2026-11-12): the List/Grid choice is kept apart for Home's Recents (rows to start) and the Recents page (cards to start). Shared with you joins the sidebar under Starred, with its count: every file shared with you (SharedWithYouResources), grouped by when it was shared, each saying who shared it ("From <maker>" when Figma doesn't say who)

Shared with you and Drafts (all and each team's) are one grid, newest first, with no date headings or month rail; dates from another year carry the year

Shared with you and Drafts (every Drafts page shares one) each have their own List/Grid switch in the filter bar, cards to start
