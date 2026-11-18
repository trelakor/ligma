// Draws release notes from CHANGELOG.md (src-tauri/src/whats_new.rs), for
// the what's new prompt (prompt.html) and Settings > What's New. Each note
// is one Markdown list item; `code` is drawn as code and the rest as text

// A list of `notes`
window.ligmaReleaseNotes = (notes) => {
  const list = document.createElement("ul");
  list.className = "release-notes";
  for (const note of notes) {
    const item = document.createElement("li");
    for (const part of note.split(/(`[^`]+`)/)) {
      if (/^`[^`]+`$/.test(part)) item.append(Object.assign(document.createElement("code"), { textContent: part.slice(1, -1) }));
      else if (part) item.append(part);
    }
    list.append(item);
  }
  return list;
};

// Switches a release's notes describe that start off, by version: the
// switch's id in settings.html, its name, and whether it's Mac only. Each
// gets a button under the notes that shows it in Settings
(() => {
  const SETTINGS = {
    "0.45.0": [["effects-library", "Effects"]],
    "0.44.0": [["frame-colors", "Color frames"]],
    "0.43.0": [["image-bar-icons", "Icons only"]],
    "0.26.0": [["use-background-model", "BiRefNet", true]],
    "0.23.0": [["classic-home", "Classic home page"], ["actions-plugins-tab", "Open on Plugins & widgets"]],
    "0.22.0": [["layer-previews", "Layer previews"], ["discord-hide-files", "Hide files by default"]],
    "0.12.0": [["discord-invites", "Invites to files"]],
    "0.8.0": [["restore", "Reopen tabs from last time"]],
    "0.4.0": [["snapshots", "Reload snapshots", true]],
  };
  const mac = /Mac/.test(navigator.userAgent);

  // Buttons for release `version`'s switches, each calling `open(id)`, or
  // null without any
  window.ligmaReleaseSettings = (version, open) => {
    const switches = (SETTINGS[version] ?? []).filter(([, , macOnly]) => mac || !macOnly);
    if (!switches.length) return null;
    const row = document.createElement("div");
    row.className = "release-settings";
    for (const [id, name] of switches) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "button";
      button.textContent = `Show ${name} in Settings`;
      button.addEventListener("click", () => open(id));
      row.append(button);
    }
    return row;
  };
})();

// 2026-11-14 as the system writes dates, like Nov 14, 2026
window.ligmaReleaseDate = (date) => {
  const [year, month, day] = (date ?? "").split("-").map(Number);
  if (!year || !month || !day) return date ?? "";
  return new Date(year, month - 1, day).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
};
