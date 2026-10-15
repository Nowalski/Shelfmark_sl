export type ItemKind = 'Clothing' | 'Object' | 'Notecard' | 'Landmark' | 'Folder' | 'Other';

export type InventoryItem = {
  id: string;
  avatarId: string;
  parentFolderId?: string;
  liveLocationConfirmedAt?: string;
  name: string;
  path: string[];
  kind: ItemKind;
  createdAt: string;
  creatorId?: string;
  description?: string;
  tags: string[];
  confidence?: 'Exact' | 'Close' | 'Loose';
  reason?: string;
  isLink?: boolean;
  possibleDuplicate?: boolean;
  linkedTargetId?: string;
  isFavorite?: boolean;
  ownerMask?: number;
  nextOwnerMask?: number;
  candidateLabel?: 'Demo-like name' | 'Possible package' | 'Possible body-fit variant' | 'Notecard' | 'Landmark';
};

export type CandidateCounts = {
  allClues: number;
  demoLikeName: number;
  possiblePackage: number;
  bodyFitVariant: number;
  possibleDuplicate: number;
  notecard: number;
  landmark: number;
};

export type SearchHit = {
  item: InventoryItem;
  match: 'Name' | 'Folder path' | 'Tag' | 'Description';
  score: number;
};

export type SavedSearch = { label: string; query: string; kind?: ItemKind | 'Any type'; page?: 'find' | 'new' };

export type LocalPreferences = {
  savedSearches: SavedSearch[];
  itemTags: Record<string, string[]>;
  itemNotes: Record<string, string>;
  protectedItemIds: string[];
  protectedFolderIds: string[];
  collections: { id: string; name: string; itemIds: string[]; cover?: string }[];
  theme: 'light' | 'dark';
};

export type ImportSummary = {
  avatarId: string;
  cacheVersion: number;
  categoryCount: number;
  itemCount: number;
  snapshotAt: string;
  recentCount: number;
  emptyFolderCount: number;
  candidateCounts: CandidateCounts;
  inventoryCachePath?: string | null;
  cacheModifiedAtMs?: number | null;
};
