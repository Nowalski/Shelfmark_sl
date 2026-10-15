export type FolderReference = {
  id: string;
  parentId: string;
  name?: string;
  preferredType?: string;
};

export type FolderSafety = {
  isProtected: boolean;
  isMarked: boolean;
  markedBy: FolderReference | null;
  systemFolder: FolderReference | null;
};

export type SafetyItem = {
  id: string;
  parentFolderId?: string;
  isLink?: boolean;
  ownerMask?: number;
};

export type ItemSafetyReason = {
  source: 'item-mark' | 'folder-mark' | 'system-folder' | 'inventory-link' | 'no-copy';
  label: string;
};

export type ItemSafety = {
  isProtected: boolean;
  isAutomaticallyProtected: boolean;
  isItemMarked: boolean;
  reasons: ItemSafetyReason[];
};

const COPY_PERMISSION = 0x8000;

// Firestorm's preferred_type values are stable folder-type identifiers. Treat
// recognized special types as keep-safe candidates, and default unknown nonempty
// types to protected so a newer system folder does not look ordinary. Firestorm's
// plain user-folder type is "-1". Marketplace folders are deliberately included
// even where the viewer allows certain Marketplace operations.
const NOT_SPECIAL_FOLDER_TYPES = new Set(['', '-1']);

const PROTECTED_FOLDER_LABELS: Record<string, string> = {
  trash: 'Trash',
  lstndfnd: 'Lost and Found',
  current: 'Current Outfit',
  my_otfts: 'My Outfits',
  favorite: 'Favorites',
  inbox: 'Marketplace received items',
  merchant: 'Marketplace listings',
  stock: 'Marketplace stock',
  version: 'Marketplace version',
  outfit: 'Outfit folder',
};

function normalizedFolderType(value?: string): string {
  const normalized = (value ?? '').trim().toLocaleLowerCase();
  return normalized === '-1' ? normalized : normalized.replace(/[\s-]+/g, '_');
}

export function isSpecialKeepSafeFolderType(preferredType?: string): boolean {
  return !NOT_SPECIAL_FOLDER_TYPES.has(normalizedFolderType(preferredType));
}

export function folderSafetyLabel(folder: FolderReference): string {
  const type = normalizedFolderType(folder.preferredType);
  return PROTECTED_FOLDER_LABELS[type]
    ?? (type ? `Firestorm folder type “${type}”` : (folder.name?.trim() || 'System folder'));
}

export function folderSafetyReasons(safety: FolderSafety): string[] {
  const reasons: string[] = [];
  if (safety.markedBy) {
    reasons.push(safety.isMarked
      ? 'Marked keep safe on this device'
      : `Inside keep-safe folder “${safety.markedBy.name ?? 'Unnamed folder'}”`);
  }
  if (safety.systemFolder) {
    reasons.push(`System folder: ${folderSafetyLabel(safety.systemFolder)}`);
  }
  return reasons;
}

export function createFolderProtectionResolver(
  folders: readonly FolderReference[],
  protectedFolderIds: readonly string[],
  rootFolderId: string,
) {
  const foldersById = new Map(folders.map((folder) => [folder.id, folder]));
  const protectedIds = new Set(protectedFolderIds);

  function getSafety(folderId: string): FolderSafety {
    let folder = foldersById.get(folderId);
    let markedBy: FolderReference | null = null;
    let systemFolder: FolderReference | null = null;
    const visited = new Set<string>();

    for (let depth = 0; folder && depth < 256 && !visited.has(folder.id); depth += 1) {
      visited.add(folder.id);
      if (!markedBy && protectedIds.has(folder.id)) markedBy = folder;
      if (!systemFolder && isSpecialKeepSafeFolderType(folder.preferredType)) systemFolder = folder;
      if (markedBy && systemFolder) break;
      folder = folder.parentId === rootFolderId ? undefined : foldersById.get(folder.parentId);
    }

    return {
      isProtected: Boolean(markedBy || systemFolder),
      isMarked: protectedIds.has(folderId),
      markedBy,
      systemFolder,
    };
  }

  return {
    isMarked: (folderId: string) => protectedIds.has(folderId),
    getSafety,
    isProtected: (folderId: string) => getSafety(folderId).isProtected,
  };
}

export function classifyItemSafety(
  item: SafetyItem,
  protectedItemIds: readonly string[] | ReadonlySet<string>,
  folderSafety: Pick<ReturnType<typeof createFolderProtectionResolver>, 'getSafety'>,
): ItemSafety {
  const reasons: ItemSafetyReason[] = [];
  const isItemMarked = 'has' in protectedItemIds ? protectedItemIds.has(item.id) : protectedItemIds.includes(item.id);

  if (isItemMarked) reasons.push({ source: 'item-mark', label: 'Marked keep safe on this device' });
  if (item.isLink) reasons.push({ source: 'inventory-link', label: 'Inventory link' });
  if (item.ownerMask !== undefined && (item.ownerMask & COPY_PERMISSION) === 0) {
    reasons.push({ source: 'no-copy', label: 'No-copy permission recorded in this snapshot' });
  }

  if (item.parentFolderId) {
    const parentSafety = folderSafety.getSafety(item.parentFolderId);
    if (parentSafety.markedBy) {
      reasons.push({
        source: 'folder-mark',
        label: `Inside keep-safe folder “${parentSafety.markedBy.name ?? 'Unnamed folder'}”`,
      });
    }
    if (parentSafety.systemFolder) {
      reasons.push({
        source: 'system-folder',
        label: `Inside ${folderSafetyLabel(parentSafety.systemFolder)}`,
      });
    }
  }

  return {
    isProtected: reasons.length > 0,
    isAutomaticallyProtected: reasons.some((reason) => reason.source !== 'item-mark'),
    isItemMarked,
    reasons,
  };
}
