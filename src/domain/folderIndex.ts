export type FolderSummary = { id: string; parentId: string; name: string; preferredType: string };

export const INVENTORY_ROOT_ID = '00000000-0000-0000-0000-000000000000';

export function getFolderTrail(folderId: string | null, byId: ReadonlyMap<string, FolderSummary>): FolderSummary[] {
  const trail: FolderSummary[] = [];
  const visited = new Set<string>();
  let current = folderId ? byId.get(folderId) : undefined;
  while (current && trail.length < 256 && !visited.has(current.id)) {
    visited.add(current.id);
    trail.push(current);
    current = current.parentId === INVENTORY_ROOT_ID ? undefined : byId.get(current.parentId);
  }
  return trail.reverse();
}

// Share one lookup across breadcrumbs, folder search, and directory navigation.
// Broken or cyclic cache ancestry must not produce an unbounded trail.
export function createFolderIndex(folders: readonly FolderSummary[]) {
  const byId = new Map(folders.map((folder) => [folder.id, folder]));
  const childrenByParent = new Map<string, FolderSummary[]>();
  for (const folder of folders) {
    const children = childrenByParent.get(folder.parentId);
    if (children) children.push(folder);
    else childrenByParent.set(folder.parentId, [folder]);
  }
  function getTrail(folderId: string | null): FolderSummary[] {
    return getFolderTrail(folderId, byId);
  }
  const pathLabels = new Map<string, string>();
  const searchEntries = folders.map((folder) => {
    const path = getTrail(folder.id).map((part) => part.name).join(' / ');
    pathLabels.set(folder.id, path);
    return { folder, searchText: path.toLocaleLowerCase() };
  });
  return { byId, childrenByParent, pathLabels, searchEntries, getTrail };
}
