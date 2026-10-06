/** The folder a torrent is saved in: the parent of its own `<name>` folder, if it has one. */
export function saveFolderOf(outputFolder: string, name?: string | null): string {
  const trimmed = outputFolder.replace(/[\\/]+$/, "");
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  if (name && cut > 0 && trimmed.slice(cut + 1) === name) {
    return trimmed.slice(0, cut);
  }
  return outputFolder;
}
