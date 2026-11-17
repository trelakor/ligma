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

// 2026-11-14 as the system writes dates, like Nov 14, 2026
window.ligmaReleaseDate = (date) => {
  const [year, month, day] = (date ?? "").split("-").map(Number);
  if (!year || !month || !day) return date ?? "";
  return new Date(year, month - 1, day).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
};
