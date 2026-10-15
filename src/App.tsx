import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { KeyboardEvent as ReactKeyboardEvent, MouseEvent as ReactMouseEvent } from 'react';
import {
  Archive, ArrowDownWideNarrow, ArrowRight, Bookmark, ChevronDown, CircleAlert, CircleCheck, CircleHelp, Clipboard, Download,
  ExternalLink, Folder, FolderOpen, FolderTree, House, ImagePlus, Layers3, Moon, PackageOpen, Pencil, Play, Plus, Search, Settings2,
  RefreshCw, Shield, ShieldCheck, Sparkles, Star, Sun, CloudOff, X,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open, save } from '@tauri-apps/plugin-dialog';
import { writeTextFile } from '@tauri-apps/plugin-fs';
import { relaunch } from '@tauri-apps/plugin-process';
import { check, type Update } from '@tauri-apps/plugin-updater';
import { candidateReason, searchItems } from './domain/search';
import { readAvatarLabels, saveAvatarLabel } from './domain/avatarLabels';
import { collectionImageKey, itemImageKey, listLocalImages, readLocalImage, readLocalImages, removeLocalImage, replaceLocalImages, saveLocalImage } from './domain/mediaStore';
import { classifyItemSafety, createFolderProtectionResolver, folderSafetyLabel, folderSafetyReasons } from './domain/folderProtection';
import { avatarTagsFromBackup, collectProfileStorage, createProfileBackup, decodeProfileImage, MAX_PROFILE_FILE_CHARS, parseProfileBackup, replaceProfileStorage, withProfileStorageOverrides, type ProfileBackup } from './domain/profileBackup';
import type { CandidateCounts, ImportSummary, InventoryItem, ItemKind, LocalPreferences, SavedSearch } from './domain/types';
import { sampleItems } from './data/sample';
import { normalizeLocalPreferences } from './domain/preferences';
import { useDialogFocus } from './domain/useDialogFocus';
import { resizeImage } from './domain/imagePreparation';
import { createFolderIndex, getFolderTrail, INVENTORY_ROOT_ID, type FolderSummary } from './domain/folderIndex';

type Page = 'home' | 'find' | 'folders' | 'empty-folders' | 'new' | 'collections' | 'review' | 'settings';
type AvatarOption = { id: string; label: string; itemCount: number; categoryCount?: number; emptyFolderCount?: number; cacheVersion?: number; snapshotAt?: string; recentCount?: number; candidateCounts?: CandidateCounts; inventoryCachePath?: string | null; cacheModifiedAtMs?: number | null };
type FolderContentsResult = { folders: FolderSummary[]; items: any[]; hasMore: boolean; totalItemCount: number };
type EmptyFoldersPageResult = { folders: FolderSummary[]; hasMore: boolean; totalCount: number };
type InventoryLinkTarget = { id: string; name: string; path: string[]; parentFolderId: string; isFolder: boolean };
type CandidateFilter = 'All clues' | 'Demo-like name' | 'Possible package' | 'Possible body-fit variant' | 'Possible duplicate' | 'Notecard' | 'Landmark';
type SortMode = 'Relevance' | 'Name A–Z' | 'Newest first';
type CacheCandidate = { path: string; fileName: string; avatarId: string; bytes: number; modifiedAtMs: number };
type FirestormDiscovery = { viewerRunning: boolean | null; cacheDirectories: string[]; candidates: CacheCandidate[]; avatarNames: Record<string, string> };
type StockLeapStatus = { phase: string; connected: boolean; avatarId: string | null; apiNames: string[]; inventoryWindowPaths: string[]; inventoryPathsScanned: boolean; message: string };

function sameStockLeapStatus(left: StockLeapStatus, right: StockLeapStatus) {
  return left.phase === right.phase
    && left.connected === right.connected
    && left.avatarId === right.avatarId
    && left.inventoryPathsScanned === right.inventoryPathsScanned
    && left.message === right.message
    && left.apiNames.length === right.apiNames.length
    && left.apiNames.every((name, index) => name === right.apiNames[index])
    && left.inventoryWindowPaths.length === right.inventoryWindowPaths.length
    && left.inventoryWindowPaths.every((path, index) => path === right.inventoryWindowPaths[index]);
}
type StockLeapInspection = { avatarId: string; apiNames: string[]; inventoryWindowPaths: string[]; inventoryPathsScanned: boolean };
type StockLeapApiDescription = { api: string; result: unknown; error?: string };
type StockLeapViewInspection = { name: string; result: unknown; error?: string };
type StockLeapMoveResult = { item: { id: string; name: string; parentId: string; assetType: string }; previousParentId: string; previousParentName: string; destinationName: string; changed: boolean };
type MoveFeedback = { itemId: string; state: 'pending' | 'success' | 'error'; message: string };
type ImportProgress = { stage: string; completed: number; total: number | null };
type RemoteData = { avatarId: string; query: string; kind: ItemKind | null; page: Page; candidateFilter: CandidateFilter; collectionView: string | null; sortMode: SortMode; snapshotRevision: number; items: InventoryItem[]; hasMore: boolean };
type LiveLocationOverride = { avatarId: string; item: InventoryItem; previousParentFolderId: string; confirmedAt: string };
type ItemContextMenuState = { item: InventoryItem; x: number; y: number };

const CANDIDATE_FILTERS: CandidateFilter[] = ['All clues', 'Demo-like name', 'Possible package', 'Possible body-fit variant', 'Possible duplicate', 'Notecard', 'Landmark'];
// Stock LEAP cannot acknowledge or clear the viewer's inventory clipboard.
// Keep ordinary-folder moves unavailable until a non-clipboard route exists.
const STOCK_LEAP_ORDINARY_FOLDER_MOVES_ENABLED = false;
const CANDIDATE_LABELS: Record<CandidateFilter, string> = {
  'All clues': 'All clues',
  'Demo-like name': 'Demo-like names',
  'Possible package': 'Possible packages',
  'Possible body-fit variant': 'Body fit / size clues',
  'Possible duplicate': 'Possible duplicate names',
  Notecard: 'Notecards',
  Landmark: 'Landmarks',
};
const SORT_MODES: SortMode[] = ['Relevance', 'Name A–Z', 'Newest first'];
const INVENTORY_PAGE_SIZE = 100;
const EMPTY_ITEMS: InventoryItem[] = [];
const EMPTY_FOLDERS: FolderSummary[] = [];
const DATE_FORMATTER = new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'short', year: 'numeric' });
const INVENTORY_UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const DEVELOPMENT_BUILD = import.meta.env.DEV;

const DEFAULT_PREFS: LocalPreferences = {
  savedSearches: [
    { label: 'Legacy outerwear', query: 'legacy jacket', kind: 'Any type', page: 'find' },
    { label: 'Recently added', query: '', kind: 'Any type', page: 'new' },
  ],
  itemTags: {},
  itemNotes: {},
  protectedItemIds: [],
  protectedFolderIds: [],
  collections: [],
  theme: 'light',
};

const KIND_OPTIONS: (ItemKind | 'Any type')[] = ['Any type', 'Clothing', 'Object', 'Notecard', 'Landmark', 'Folder', 'Other'];
const PERMISSION_FLAGS = [
  { label: 'Copy', mask: 0x8000 },
  { label: 'Modify', mask: 0x4000 },
  { label: 'Transfer', mask: 0x2000 },
  { label: 'Move', mask: 0x80000 },
] as const;

function permissionSummary(mask?: number): string {
  if (mask === undefined) return 'Not recorded in this snapshot';
  const flags = PERMISSION_FLAGS.filter(({ mask: flag }) => (mask & flag) !== 0).map(({ label }) => label);
  return flags.length ? flags.join(' · ') : 'No listed flags set';
}

function buildSampleFolders(items: InventoryItem[]): FolderSummary[] {
  const folders = new Map<string, FolderSummary>();
  for (const item of items) {
    for (let index = 0; index < item.path.length; index += 1) {
      const parts = item.path.slice(0, index + 1);
      const parentParts = parts.slice(0, -1);
      const id = `sample-folder:${JSON.stringify(parts)}`;
      folders.set(id, {
        id,
        parentId: parentParts.length ? `sample-folder:${JSON.stringify(parentParts)}` : INVENTORY_ROOT_ID,
        name: parts[parts.length - 1],
        preferredType: '',
      });
    }
  }
  folders.set('sample-folder:["Clothing","To sort"]', {
    id: 'sample-folder:["Clothing","To sort"]',
    parentId: 'sample-folder:["Clothing"]',
    name: 'To sort',
    preferredType: '',
  });
  return [...folders.values()].sort((a, b) => a.name.localeCompare(b.name));
}

function folderTrail(folderId: string | null, folders: FolderSummary[]): FolderSummary[] {
  return getFolderTrail(folderId, new Map(folders.map((folder) => [folder.id, folder])));
}

function isDesktop(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

function isInventoryUuid(id: string): boolean {
  return id !== INVENTORY_ROOT_ID && INVENTORY_UUID_PATTERN.test(id);
}

function supportsStockLeapMove(avatarId: string): boolean {
  const grid = avatarId.split('@')[1]?.toLocaleLowerCase();
  return grid === 'secondlife' || grid === 'agni';
}

function viewerSnapshotFor(avatars: AvatarOption[], activeAvatarId: string | null): AvatarOption | null {
  if (!activeAvatarId) return null;
  const normalizedAvatarId = activeAvatarId.toLocaleLowerCase();
  return avatars.find((entry) => supportsStockLeapMove(entry.id)
    && entry.id.split('@')[0].toLocaleLowerCase() === normalizedAvatarId) ?? null;
}

function readPrefs(avatarId: string): LocalPreferences {
  try {
    const value = localStorage.getItem(`shelfmark:${avatarId}`);
    const parsed = value ? JSON.parse(value) : {};
    const stored = normalizeLocalPreferences(parsed, DEFAULT_PREFS);
    if (avatarId === 'sample-avatar' && localStorage.getItem('shelfmark:sample-seed-complete') !== 'true') {
      const samplePrefs = stored.collections.length ? stored : { ...stored, collections: [{ id: 'sample-winter', name: 'Winter pieces to revisit', itemIds: ['sm-001', 'sm-004', 'sm-009'] }] };
      localStorage.setItem(`shelfmark:${avatarId}`, JSON.stringify(samplePrefs));
      localStorage.setItem('shelfmark:sample-seed-complete', 'true');
      return samplePrefs;
    }
    return stored;
  } catch { return DEFAULT_PREFS; }
}

function shortId(id: string): string {
  if (id === 'sample-avatar') return 'Sample avatar';
  const [ownerId, gridId] = id.split('@');
  const identity = `${ownerId.slice(0, 4)}…${ownerId.slice(-4)}`;
  return gridId && gridId !== 'secondlife' ? `Avatar · ${identity} · ${gridId}` : `Avatar · ${identity}`;
}

function formatDate(value: string): string {
  if (!value) return 'Date unavailable';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return 'Date unavailable';
  return DATE_FORMATTER.format(date);
}

function relativeTime(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return 'date unavailable';
  const days = Math.max(0, Math.round((Date.now() - date.getTime()) / 86_400_000));
  return days === 0 ? 'today' : days === 1 ? 'yesterday' : `${days} days ago`;
}

function fromLocalIndex(row: any): InventoryItem {
  return {
    id: row.id,
    avatarId: row.avatarId,
    parentFolderId: row.parentFolderId ?? undefined,
    name: row.name,
    path: row.path,
    kind: row.kind as ItemKind,
    createdAt: row.createdAt ?? '',
    creatorId: row.creatorId ?? undefined,
    ownerMask: row.ownerMask ?? undefined,
    nextOwnerMask: row.nextOwnerMask ?? undefined,
    description: row.description,
    tags: row.tags ?? [],
    isLink: row.isLink,
    possibleDuplicate: row.possibleDuplicate ?? false,
    linkedTargetId: row.linkedTargetId ?? undefined,
    isFavorite: row.isFavorite ?? false,
    candidateLabel: row.candidateLabel ?? undefined,
  };
}

function App() {
  const desktop = useMemo(isDesktop, []);
  const searchShortcut = useMemo(() => /Mac|iPhone|iPad/.test(navigator.platform) ? '⌘ K' : 'Ctrl K', []);
  const searchRef = useRef<HTMLInputElement>(null);
  const itemImageInputRef = useRef<HTMLInputElement>(null);
  const profileBackupInputRef = useRef<HTMLInputElement>(null);
  const helpTriggerRef = useRef<HTMLButtonElement>(null);
  const mobileDetailCloseRef = useRef<HTMLButtonElement>(null);
  const startupImportTriggeredRef = useRef(false);
  const [page, setPage] = useState<Page>(() => isDesktop() ? 'home' : 'find');
  const activePageRef = useRef(page);
  activePageRef.current = page;
  const [query, setQuery] = useState('');
  const [kind, setKind] = useState<ItemKind | 'Any type'>('Any type');
  const [kindMenu, setKindMenu] = useState(false);
  const [candidateFilter, setCandidateFilter] = useState<CandidateFilter>('All clues');
  const [sortMode, setSortMode] = useState<SortMode>('Relevance');
  const [sortMenu, setSortMenu] = useState(false);
  const [avatar, setAvatar] = useState<AvatarOption>({ id: 'sample-avatar', label: 'Sample avatar', itemCount: sampleItems.length, emptyFolderCount: 1 });
  const activeAvatarIdRef = useRef(avatar.id);
  activeAvatarIdRef.current = avatar.id;
  const [avatars, setAvatars] = useState<AvatarOption[]>([]);
  const [avatarListLoaded, setAvatarListLoaded] = useState(false);
  const [avatarListError, setAvatarListError] = useState('');
  const [avatarListRetry, setAvatarListRetry] = useState(0);
  const avatarListRequestVersionRef = useRef(0);
  const [avatarLabels, setAvatarLabels] = useState<Record<string, string>>(() => readAvatarLabels());
  const avatarLabelsRef = useRef(avatarLabels);
  avatarLabelsRef.current = avatarLabels;
  const [firestormAvatarNames, setFirestormAvatarNames] = useState<Record<string, string>>({});
  const [avatarLabelEditorOpen, setAvatarLabelEditorOpen] = useState(false);
  const [avatarLabelDraft, setAvatarLabelDraft] = useState('');
  const [folderTree, setFolderTree] = useState<FolderSummary[]>([]);
  const [folderTreeLoadedFor, setFolderTreeLoadedFor] = useState<string | null>(null);
  const [folderTreeLoading, setFolderTreeLoading] = useState(false);
  const [folderTreeError, setFolderTreeError] = useState('');
  const [folderReadRevision, setFolderReadRevision] = useState(0);
  const [selectedFolderId, setSelectedFolderId] = useState<string | null>(null);
  const activeSelectedFolderIdRef = useRef(selectedFolderId);
  activeSelectedFolderIdRef.current = selectedFolderId;
  const [folderSearch, setFolderSearch] = useState('');
  const [folderItems, setFolderItems] = useState<InventoryItem[]>([]);
  const [folderItemsLoading, setFolderItemsLoading] = useState(false);
  const [folderItemsLoadingMore, setFolderItemsLoadingMore] = useState(false);
  const [folderItemsError, setFolderItemsError] = useState('');
  const [folderItemsHasMore, setFolderItemsHasMore] = useState(false);
  const [folderItemsTotal, setFolderItemsTotal] = useState(0);
  const folderPageOffsetRef = useRef(0);
  const folderPageTokenRef = useRef<object | null>(null);
  const [emptyFolders, setEmptyFolders] = useState<FolderSummary[]>([]);
  const [emptyFoldersHasMore, setEmptyFoldersHasMore] = useState(false);
  const [emptyFoldersTotal, setEmptyFoldersTotal] = useState(0);
  const [emptyFoldersLoading, setEmptyFoldersLoading] = useState(false);
  const [emptyFoldersLoadingMore, setEmptyFoldersLoadingMore] = useState(false);
  const [emptyFoldersError, setEmptyFoldersError] = useState('');
  const [remoteData, setRemoteData] = useState<RemoteData | null>(null);
  const [liveLocationOverrides, setLiveLocationOverrides] = useState<Record<string, LiveLocationOverride>>({});
  const [remoteLoading, setRemoteLoading] = useState(false);
  const [remotePageLoading, setRemotePageLoading] = useState(false);
  const remoteRequestVersionRef = useRef(0);
  const folderRequestVersionRef = useRef(0);
  const emptyRequestVersionRef = useRef(0);
  const remotePageTokenRef = useRef<object | null>(null);
  const emptyPageTokenRef = useRef<object | null>(null);
  const [remoteError, setRemoteError] = useState('');
  const [selectedId, setSelectedId] = useState('sm-001');
  const activeSelectedIdRef = useRef('sm-001');
  activeSelectedIdRef.current = selectedId;
  const [prefs, setPrefs] = useState<LocalPreferences>(DEFAULT_PREFS);
  const [prefsLoadedFor, setPrefsLoadedFor] = useState<string | null>(null);
  const prefsLoadedForRef = useRef(prefsLoadedFor);
  prefsLoadedForRef.current = prefsLoadedFor;
  const [prefsSaveError, setPrefsSaveError] = useState('');
  const unsavedPreferencesRef = useRef(new Map<string, LocalPreferences>());
  const toastTimerRef = useRef<number | null>(null);
  const [tagDraft, setTagDraft] = useState('');
  const [tagEditor, setTagEditor] = useState(false);
  const tagSaveBusyRef = useRef(false);
  const [tagSaveBusy, setTagSaveBusy] = useState(false);
  const [tagSearchRevision, setTagSearchRevision] = useState(0);
  const [tagSyncErrors, setTagSyncErrors] = useState<Record<string, string>>({});
  const [savedSearchName, setSavedSearchName] = useState('');
  const [saveSearchOpen, setSaveSearchOpen] = useState(false);
  const [collectionDraft, setCollectionDraft] = useState('');
  const [collectionOpen, setCollectionOpen] = useState(false);
  const [newCollectionDialogOpen, setNewCollectionDialogOpen] = useState(false);
  const [moveDialogOpen, setMoveDialogOpen] = useState(false);
  const [moveDialogLoading, setMoveDialogLoading] = useState(false);
  const [moveFolderSearch, setMoveFolderSearch] = useState('');
  const [moveDestinationId, setMoveDestinationId] = useState('');
  const [moveBusy, setMoveBusy] = useState(false);
  const moveBusyRef = useRef(false);
  const moveDialogLoadTokenRef = useRef(0);
  const [itemContextMenu, setItemContextMenu] = useState<ItemContextMenuState | null>(null);
  const itemContextMenuRef = useRef<HTMLDivElement>(null);
  const itemContextMenuTriggerRef = useRef<HTMLButtonElement | null>(null);
  const [moveFeedback, setMoveFeedback] = useState<MoveFeedback | null>(null);
  const [snapshotRefreshBusy, setSnapshotRefreshBusy] = useState(false);
  const snapshotRefreshBusyRef = useRef(false);
  const [snapshotRefreshNeededFor, setSnapshotRefreshNeededFor] = useState<string | null>(null);
  const [snapshotRevision, setSnapshotRevision] = useState(0);
  const [toast, setToast] = useState('');
  const [busy, setBusy] = useState(false);
  const importBusyRef = useRef(false);
  const [viewerActionBusy, setViewerActionBusy] = useState(false);
  const viewerActionBusyRef = useRef(false);
  const [importProgress, setImportProgress] = useState<ImportProgress | null>(null);
  const [importCancellationPending, setImportCancellationPending] = useState(false);
  const [profileBusy, setProfileBusy] = useState(false);
  const profileBusyRef = useRef(false);
  const profileRestoreBusyRef = useRef(false);
  const [profileRestore, setProfileRestore] = useState<ProfileBackup | null>(null);
  const [importSummary, setImportSummary] = useState<ImportSummary | null>(null);
  const [avatarMenu, setAvatarMenu] = useState(false);
  const [collectionView, setCollectionView] = useState<string | null>(null);
  const [helpOpen, setHelpOpen] = useState(false);
  const [mobileDetailOpen, setMobileDetailOpen] = useState(false);
  const [cacheDialogOpen, setCacheDialogOpen] = useState(false);
  const [cacheCandidates, setCacheCandidates] = useState<CacheCandidate[]>([]);
  const [firestormRunning, setFirestormRunning] = useState<boolean | null>(null);
  const [stockLeapStatus, setStockLeapStatus] = useState<StockLeapStatus>({ phase: 'idle', connected: false, avatarId: null, apiNames: [], inventoryWindowPaths: [], inventoryPathsScanned: false, message: 'Stock Firestorm LEAP is not connected.' });
  const stockLeapConnectionRef = useRef({ connected: false, version: 0 });
  if (stockLeapConnectionRef.current.connected !== stockLeapStatus.connected) {
    stockLeapConnectionRef.current = { connected: stockLeapStatus.connected, version: stockLeapConnectionRef.current.version + 1 };
  }
  const [stockLeapBusy, setStockLeapBusy] = useState(false);
  const stockLeapBusyRef = useRef(false);
  const [viewerLaunchError, setViewerLaunchError] = useState('');
  const stockLeapAutoInspectStartedRef = useRef(false);
  const stockLeapAutoInspectAttemptsRef = useRef(0);
  const stockLeapAutoInspectTimerRef = useRef<number | null>(null);
  const [stockLeapAutoInspectRetry, setStockLeapAutoInspectRetry] = useState(0);
  const [stockLeapApiBusy, setStockLeapApiBusy] = useState<string | null>(null);
  const [stockLeapApiDescription, setStockLeapApiDescription] = useState<StockLeapApiDescription | null>(null);
  const [stockLeapViewName, setStockLeapViewName] = useState('');
  const [stockLeapViewBusy, setStockLeapViewBusy] = useState(false);
  const [stockLeapViewInspection, setStockLeapViewInspection] = useState<StockLeapViewInspection | null>(null);
  const [firestormCacheDirectories, setFirestormCacheDirectories] = useState<string[]>([]);
  const [selectedCacheCandidate, setSelectedCacheCandidate] = useState<CacheCandidate | null>(null);
  const selectedCacheCandidateRef = useRef(selectedCacheCandidate);
  selectedCacheCandidateRef.current = selectedCacheCandidate;
  const cacheRequestVersionRef = useRef(0);
  const cacheChooserActiveRef = useRef(false);
  const [cacheDiscoveryBusy, setCacheDiscoveryBusy] = useState(false);
  const [cacheDialogError, setCacheDialogError] = useState('');
  const [selectedImage, setSelectedImage] = useState<{ key: string; url: string } | null>(null);
  const [localMediaRevision, setLocalMediaRevision] = useState(0);
  const [photoBusy, setPhotoBusy] = useState(false);
  const photoBusyRef = useRef(false);
  const activeItemPhotoKeyRef = useRef<string | null>(null);
  const photoRequestVersionRef = useRef(0);
  const [selectedThumbnail, setSelectedThumbnail] = useState<{ key: string; url: string } | null>(null);
  const [linkTarget, setLinkTarget] = useState<InventoryLinkTarget | null>(null);
  const [linkTargetLoading, setLinkTargetLoading] = useState(false);
  const [linkTargetError, setLinkTargetError] = useState(false);
  const [collectionCoverUrls, setCollectionCoverUrls] = useState<Record<string, string>>({});
  const [pendingUpdate, setPendingUpdate] = useState<Update | null>(null);
  const pendingUpdateRef = useRef<Update | null>(null);
  const [updateStatus, setUpdateStatus] = useState('Checking for updates…');
  const [updateBusy, setUpdateBusy] = useState(false);
  const updateBusyRef = useRef(false);
  const [updateInstalling, setUpdateInstalling] = useState(false);

  useEffect(() => () => {
    cacheChooserActiveRef.current = false;
    cacheRequestVersionRef.current += 1;
  }, []);

  useEffect(() => {
    if (!desktop) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<ImportProgress>('shelfmark-import-progress', (event) => {
      setImportProgress(event.payload);
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    }).catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [desktop]);

  useEffect(() => {
    if (!desktop) return;
    let disposed = false;
    let polling = false;
    const poll = async () => {
      if (polling) return;
      polling = true;
      try {
        const status = await invoke<StockLeapStatus>('get_stock_firestorm_leap_status');
        if (!disposed) setStockLeapStatus((current) => sameStockLeapStatus(current, status) ? current : status);
      } catch { /* The browser preview has no native LEAP command. */ }
      finally { polling = false; }
    };
    void poll();
    const timer = window.setInterval(() => void poll(), 2500);
    return () => { disposed = true; window.clearInterval(timer); };
  }, [desktop]);

  async function startStockFirestormLeap() {
    if (!desktop || stockLeapBusyRef.current) return;
    setViewerLaunchError('');
    stockLeapBusyRef.current = true;
    setStockLeapBusy(true);
    try {
      const detectedPath = await invoke<string | null>('discover_stock_firestorm_executable');
      const executablePath = detectedPath ?? await open({
        title: 'Choose stock Firestorm',
        multiple: false,
        directory: false,
        filters: [{ name: 'Firestorm viewer', extensions: ['exe'] }],
      });
      if (typeof executablePath !== 'string') return;
      const status = await invoke<StockLeapStatus>('start_stock_firestorm_leap', { executablePath });
      setStockLeapStatus(status);
      pushToast(detectedPath ? 'Detected Firestorm is starting with the Shelfmark LEAP helper.' : 'Stock Firestorm is starting with the Shelfmark LEAP helper.');
    } catch (error) {
      setViewerLaunchError(String(error));
      pushToast(String(error));
    } finally {
      stockLeapBusyRef.current = false;
      setStockLeapBusy(false);
    }
  }

  async function inspectStockFirestormLeap(showFailureToast = true): Promise<boolean> {
    if (!desktop || !stockLeapStatus.connected || stockLeapBusyRef.current) return false;
    if (showFailureToast && stockLeapAutoInspectTimerRef.current !== null) {
      window.clearTimeout(stockLeapAutoInspectTimerRef.current);
      stockLeapAutoInspectTimerRef.current = null;
      stockLeapAutoInspectAttemptsRef.current = 0;
    }
    stockLeapBusyRef.current = true;
    setStockLeapBusy(true);
    const connectionVersion = stockLeapConnectionRef.current.version;
    try {
      const inspection = await invoke<StockLeapInspection>('inspect_stock_firestorm_leap');
      const viewerNames = await invoke<Record<string, string>>('resolve_firestorm_avatar_names', { avatarIds: [`${inspection.avatarId}@secondlife`] }).catch((): Record<string, string> => ({}));
      if (!stockLeapConnectionRef.current.connected || stockLeapConnectionRef.current.version !== connectionVersion) return false;
      setFirestormAvatarNames((current) => ({ ...current, ...viewerNames }));
      setStockLeapStatus((current) => ({ ...current, avatarId: inspection.avatarId, apiNames: inspection.apiNames, inventoryWindowPaths: inspection.inventoryWindowPaths, inventoryPathsScanned: inspection.inventoryPathsScanned, message: 'Connected to stock Firestorm. LEAP APIs and active avatar were read.' }));
      pushToast(`LEAP read ${inspection.apiNames.length} APIs for ${viewerNames[`${inspection.avatarId}@secondlife`] ?? inspection.avatarId}.`);
      return true;
    } catch (error) {
      if (showFailureToast) pushToast(String(error));
      return false;
    } finally {
      stockLeapBusyRef.current = false;
      setStockLeapBusy(false);
    }
  }

  useEffect(() => {
    if (!desktop) return;
    if (!stockLeapStatus.connected) {
      if (stockLeapAutoInspectTimerRef.current !== null) window.clearTimeout(stockLeapAutoInspectTimerRef.current);
      stockLeapAutoInspectTimerRef.current = null;
      stockLeapAutoInspectAttemptsRef.current = 0;
      stockLeapAutoInspectStartedRef.current = false;
      return;
    }
    if (stockLeapBusy || stockLeapAutoInspectStartedRef.current) return;
    stockLeapAutoInspectStartedRef.current = true;
    const connectionVersion = stockLeapConnectionRef.current.version;
    void inspectStockFirestormLeap(false).then((inspected) => {
      if (inspected || !stockLeapConnectionRef.current.connected || stockLeapConnectionRef.current.version !== connectionVersion) return;
      const retryDelaysMs = [2000, 5000, 10000, 20000, 30000];
      const attempt = stockLeapAutoInspectAttemptsRef.current;
      if (attempt >= retryDelaysMs.length) {
        pushToast('Shelfmark could not inspect Firestorm automatically. Open Settings and retry Inspect.');
        return;
      }
      stockLeapAutoInspectAttemptsRef.current += 1;
      stockLeapAutoInspectTimerRef.current = window.setTimeout(() => {
        stockLeapAutoInspectTimerRef.current = null;
        stockLeapAutoInspectStartedRef.current = false;
        setStockLeapAutoInspectRetry((value) => value + 1);
      }, retryDelaysMs[attempt]);
    });
  }, [desktop, stockLeapBusy, stockLeapStatus.connected, stockLeapAutoInspectRetry]);

  useEffect(() => () => {
    if (stockLeapAutoInspectTimerRef.current !== null) window.clearTimeout(stockLeapAutoInspectTimerRef.current);
  }, []);

  async function scanStockFirestormInventoryUi() {
    if (!desktop || !stockLeapStatus.connected || stockLeapBusyRef.current || stockLeapApiBusy) return;
    stockLeapBusyRef.current = true;
    setStockLeapBusy(true);
    const connectionVersion = stockLeapConnectionRef.current.version;
    try {
      const paths = await invoke<string[]>('scan_stock_firestorm_inventory_ui');
      if (!stockLeapConnectionRef.current.connected || stockLeapConnectionRef.current.version !== connectionVersion) return;
      setStockLeapStatus((current) => ({ ...current, inventoryWindowPaths: paths, inventoryPathsScanned: true, message: `Located ${paths.length} inventory UI controls in Firestorm.` }));
      pushToast(`Found ${paths.length} inventory UI controls.`);
    } catch (error) {
      pushToast(String(error));
    } finally {
      stockLeapBusyRef.current = false;
      setStockLeapBusy(false);
    }
  }

  async function inspectStockFirestormInventoryView() {
    const name = stockLeapViewName.trim();
    if (!desktop || !stockLeapStatus.connected || !name || stockLeapBusy || stockLeapViewBusy || stockLeapApiBusy) return;
    setStockLeapViewBusy(true);
    setStockLeapViewInspection(null);
    try {
      const result = await invoke<unknown>('inspect_stock_firestorm_inventory_view', { name });
      setStockLeapViewInspection({ name, result });
    } catch (error) {
      setStockLeapViewInspection({ name, result: null, error: String(error) });
    } finally {
      setStockLeapViewBusy(false);
    }
  }

  async function describeStockFirestormLeapApi(api: string) {
    if (!desktop || !stockLeapStatus.connected || stockLeapApiBusy) return;
    setStockLeapApiBusy(api);
    setStockLeapApiDescription(null);
    try {
      const result = await invoke<unknown>('describe_stock_firestorm_leap_api', { api });
      setStockLeapApiDescription({ api, result });
    } catch (error) {
      setStockLeapApiDescription({ api, result: null, error: String(error) });
    } finally {
      setStockLeapApiBusy(null);
    }
  }

  async function checkForUpdates() {
    if (!desktop || updateBusyRef.current) return;
    updateBusyRef.current = true;
    setUpdateBusy(true);
    setUpdateStatus('Checking GitHub Releases…');
    try {
      const next = await check();
      const previous = pendingUpdateRef.current;
      pendingUpdateRef.current = next;
      setPendingUpdate(next);
      void previous?.close().catch(() => undefined);
      setUpdateStatus(next ? `Version ${next.version} is available.` : 'Shelfmark is up to date.');
    } catch {
      setUpdateStatus('Could not check for updates. Check your connection and try again.');
    } finally {
      updateBusyRef.current = false;
      setUpdateBusy(false);
    }
  }

  async function installAvailableUpdate() {
    const update = pendingUpdateRef.current;
    if (!update || updateBusyRef.current) return;
    if (importBusyRef.current || moveBusyRef.current || photoBusyRef.current || profileBusyRef.current || tagSaveBusyRef.current || stockLeapBusyRef.current || viewerActionBusyRef.current || cacheDiscoveryBusy || snapshotRefreshBusyRef.current || unsavedPreferencesRef.current.size) {
      pushToast('Finish current changes and retry saving local preferences before installing an update.');
      return;
    }
    updateBusyRef.current = true;
    setUpdateInstalling(true);
    setUpdateBusy(true);
    setUpdateStatus(`Preparing version ${update.version}…`);
    let downloaded = 0;
    let contentLength = 0;
    let installed = false;
    try {
      await update.downloadAndInstall((event) => {
        if (event.event === 'Started') {
          contentLength = event.data.contentLength ?? 0;
          setUpdateStatus('Downloading update…');
        } else if (event.event === 'Progress') {
          downloaded += event.data.chunkLength;
          if (contentLength > 0) {
            const percent = Math.min(100, Math.round((downloaded / contentLength) * 100));
            setUpdateStatus(`Downloading update · ${percent}%`);
          }
        } else if (event.event === 'Finished') {
          setUpdateStatus('Download complete. Installing update…');
        }
      });
      installed = true;
      pendingUpdateRef.current = null;
      setPendingUpdate(null);
      void update.close().catch(() => undefined);
      setUpdateStatus('Update installed. Restarting Shelfmark…');
      await relaunch();
    } catch {
      setUpdateStatus(installed
        ? 'The update was installed, but automatic restart failed. Close and reopen Shelfmark.'
        : 'The update could not be installed. Try again or download it from GitHub Releases.');
    } finally {
      updateBusyRef.current = false;
      setUpdateInstalling(false);
      setUpdateBusy(false);
    }
  }

  useEffect(() => {
    if (!desktop || DEVELOPMENT_BUILD) {
      setUpdateStatus(desktop ? 'Update checks are available in signed release builds.' : 'The browser app updates when its hosted site is redeployed.');
      return;
    }
    let cancelled = false;
    updateBusyRef.current = true;
    setUpdateBusy(true);
    void check().then(async (next) => {
      if (cancelled) {
        await next?.close();
        return;
      }
      pendingUpdateRef.current = next;
      setPendingUpdate(next);
      setUpdateStatus(next ? `Version ${next.version} is available.` : 'Shelfmark is up to date.');
    }).catch(() => {
      if (!cancelled) setUpdateStatus('Automatic check unavailable. You can check again here.');
    }).finally(() => {
      if (!cancelled) { updateBusyRef.current = false; setUpdateBusy(false); }
    });
    return () => {
      cancelled = true;
      const current = pendingUpdateRef.current;
      pendingUpdateRef.current = null;
      void current?.close().catch(() => undefined);
    };
  }, [desktop]);

  function closeMobileDetail() {
    setMobileDetailOpen(false);
    window.requestAnimationFrame(() => document.querySelector<HTMLButtonElement>('.item-row[aria-selected="true"]')?.focus());
  }

  useEffect(() => {
    const stored = unsavedPreferencesRef.current.get(avatar.id) ?? readPrefs(avatar.id);
    setPrefs(stored);
    setPrefsLoadedFor(avatar.id);
    setPrefsSaveError(unsavedPreferencesRef.current.size ? 'Some local changes are not saved. Export a profile before closing Shelfmark.' : '');
    setCollectionView(null);
    setCollectionOpen(false);
    setNewCollectionDialogOpen(false);
    setMoveDialogOpen(false);
    moveDialogLoadTokenRef.current += 1;
    setMoveDialogLoading(false);
    setMoveFeedback(null);
    setItemContextMenu(null);
    setTagEditor(false);
    setMobileDetailOpen(false);
    document.documentElement.dataset.theme = stored.theme;
  }, [avatar.id]);

  useEffect(() => {
    if (mobileDetailOpen) mobileDetailCloseRef.current?.focus();
  }, [mobileDetailOpen]);

  useEffect(() => {
    if (prefsLoadedFor !== avatar.id || profileRestoreBusyRef.current) return;
    try {
      localStorage.setItem(`shelfmark:${avatar.id}`, JSON.stringify(prefs));
      unsavedPreferencesRef.current.delete(avatar.id);
      setPrefsSaveError(unsavedPreferencesRef.current.size ? 'Some local changes are not saved. Export a profile before closing Shelfmark.' : '');
    } catch {
      unsavedPreferencesRef.current.set(avatar.id, prefs);
      setPrefsSaveError('Your latest notes, tags, or collections could not be saved. Keep Shelfmark open and export a profile before closing it.');
    }
    document.documentElement.dataset.theme = prefs.theme;
  }, [avatar.id, prefs, prefsLoadedFor]);

  useEffect(() => {
    if (page !== 'collections' || collectionView) {
      setCollectionCoverUrls({});
      return;
    }
    let cancelled = false;
    const createdUrls: string[] = [];
    setCollectionCoverUrls({});
    void readLocalImages(prefs.collections.map((board) => collectionImageKey(avatar.id, board.id))).then((images) => {
      if (cancelled) return;
      const entries = prefs.collections.flatMap((board) => {
        const image = images.get(collectionImageKey(avatar.id, board.id));
        if (!image) return [];
        const url = URL.createObjectURL(image);
        createdUrls.push(url);
        return [[board.id, url]];
      });
      setCollectionCoverUrls(Object.fromEntries(entries));
    }).catch(() => undefined);
    return () => {
      cancelled = true;
      createdUrls.forEach((url) => URL.revokeObjectURL(url));
    };
  }, [avatar.id, collectionView, localMediaRevision, page, prefs.collections]);

  useEffect(() => {
    if (!desktop) return;
    let disposed = false;
    const version = ++avatarListRequestVersionRef.current;
    setAvatarListError('');
    setAvatarListLoaded(false);
    void invoke<{ avatarId: string; cacheVersion: number; categoryCount: number; itemCount: number; importedAt: string; recentCount: number; emptyFolderCount: number; candidateCounts: CandidateCounts; inventoryCachePath: string | null; cacheModifiedAtMs: number | null }[]>('list_avatars').then((found) => {
      if (disposed || version !== avatarListRequestVersionRef.current) return;
      const labels = readAvatarLabels();
      setAvatarLabels(labels);
      const mapped = found.map(({ avatarId, importedAt, ...item }) => ({ ...item, label: labels[avatarId] ?? shortId(avatarId), id: avatarId, snapshotAt: importedAt }));
      setAvatars(mapped);
      if (mapped.length) setAvatar((current) => current.id === 'sample-avatar' ? mapped[0] : current);
      setAvatarListLoaded(true);
      // Avatar names are optional; the saved inventory snapshot stays local.
      if (found.length) void invoke<Record<string, string>>('resolve_firestorm_avatar_names', { avatarIds: found.map(({ avatarId }) => avatarId) })
        .then((names) => {
          if (disposed || version !== avatarListRequestVersionRef.current) return;
          setFirestormAvatarNames((current) => ({ ...current, ...names }));
          const currentLabels = readAvatarLabels();
          const labelFor = (option: AvatarOption) => currentLabels[option.id] ?? names[option.id] ?? option.label;
          setAvatars((current) => current.map((option) => ({ ...option, label: labelFor(option) })));
          setAvatar((current) => ({ ...current, label: labelFor(current) }));
        }).catch(() => undefined);
    }).catch((error) => {
      if (disposed || version !== avatarListRequestVersionRef.current) return;
      setAvatarListError(`Could not open your local snapshot list: ${String(error)}`);
      setAvatarListLoaded(true);
    });
    return () => { disposed = true; };
  }, [avatarListRetry, desktop]);

  useEffect(() => {
    if (!desktop || !stockLeapStatus.connected || !avatarListLoaded || avatarListError || avatars.length > 0 || startupImportTriggeredRef.current) return;
    startupImportTriggeredRef.current = true;
    void importCache();
  }, [avatarListError, avatarListLoaded, avatars.length, desktop, stockLeapStatus.connected]);

  useEffect(() => {
    if (page !== 'folders' && page !== 'empty-folders') return;
    let cancelled = false;
    setFolderTreeLoading(true);
    setFolderTreeError('');
    setFolderItems([]);
    setFolderItemsTotal(0);
    if (avatar.id === 'sample-avatar') {
      setFolderTree(buildSampleFolders(sampleItems));
      setFolderTreeLoadedFor(avatar.id);
      setFolderTreeLoading(false);
      return () => { cancelled = true; };
    }
    setFolderTree([]);
    setFolderTreeLoadedFor(null);
    invoke<FolderSummary[]>('list_inventory_folders', { avatarId: avatar.id })
      .then((folders) => {
        if (cancelled) return;
        setFolderTree(folders);
        setFolderTreeLoadedFor(avatar.id);
      })
      .catch((error) => { if (!cancelled) setFolderTreeError(String(error)); })
      .finally(() => { if (!cancelled) setFolderTreeLoading(false); });
    return () => { cancelled = true; };
  }, [avatar.id, page, snapshotRevision, folderReadRevision]);

  useEffect(() => {
    const requestVersion = ++emptyRequestVersionRef.current;
    emptyPageTokenRef.current = null;
    setEmptyFoldersLoadingMore(false);
    if (page !== 'empty-folders') return;
    let cancelled = false;
    setEmptyFoldersLoading(true);
    setEmptyFoldersError('');
    setEmptyFolders([]);
    if (avatar.id === 'sample-avatar') {
      const folders = buildSampleFolders(sampleItems);
      const index = createFolderIndex(folders);
      const childIds = new Set(folders.map((folder) => folder.parentId));
      const itemPaths = new Set(sampleItems.map((item) => item.path.join('\u0000')));
      const empty = folders.filter((folder) => {
        const path = index.getTrail(folder.id).map((part) => part.name).join('\u0000');
        return !childIds.has(folder.id) && !itemPaths.has(path);
      });
      setEmptyFolders(empty);
      setEmptyFoldersTotal(empty.length);
      setEmptyFoldersHasMore(false);
      setEmptyFoldersLoading(false);
      return () => { cancelled = true; };
    }
    invoke<EmptyFoldersPageResult>('list_empty_inventory_folders', {
      avatarId: avatar.id,
      offset: 0,
      limit: INVENTORY_PAGE_SIZE,
    })
      .then((result) => {
        if (cancelled || requestVersion !== emptyRequestVersionRef.current) return;
        setEmptyFolders(result.folders);
        setEmptyFoldersHasMore(result.hasMore);
        setEmptyFoldersTotal(result.totalCount);
      })
      .catch((error) => { if (!cancelled && requestVersion === emptyRequestVersionRef.current) setEmptyFoldersError(String(error)); })
      .finally(() => { if (!cancelled && requestVersion === emptyRequestVersionRef.current) setEmptyFoldersLoading(false); });
    return () => { cancelled = true; };
  }, [avatar.id, page, snapshotRevision]);

  useEffect(() => {
    folderRequestVersionRef.current += 1;
    folderPageTokenRef.current = null;
    folderPageOffsetRef.current = 0;
    setFolderItemsLoadingMore(false);
    if (page !== 'folders' || folderTreeLoading || folderTreeLoadedFor !== avatar.id) return;
    let cancelled = false;
    setFolderItemsLoading(true);
    setFolderItemsError('');
    setFolderItems([]);
    setFolderItemsHasMore(false);
    setFolderItemsTotal(0);
    if (avatar.id === 'sample-avatar') {
      const path = folderTrail(selectedFolderId, folderTree).map((folder) => folder.name);
      const items = sampleItems.filter((item) => item.path.length === path.length && path.every((part, index) => item.path[index] === part));
      setFolderItems(items);
      setFolderItemsTotal(items.length);
      setFolderItemsHasMore(false);
      setFolderItemsLoading(false);
      return () => { cancelled = true; };
    }
    invoke<FolderContentsResult>('get_inventory_folder_contents', {
      avatarId: avatar.id,
      folderId: selectedFolderId,
      offset: 0,
      limit: INVENTORY_PAGE_SIZE,
    })
      .then((result) => {
        if (cancelled) return;
        folderPageOffsetRef.current = result.items.length;
        setFolderItems(result.items.map(fromLocalIndex));
        setFolderItemsHasMore(result.hasMore);
        setFolderItemsTotal(result.totalItemCount);
      })
      .catch((error) => { if (!cancelled) { setFolderItems([]); setFolderItemsError(String(error)); } })
      .finally(() => { if (!cancelled) setFolderItemsLoading(false); });
    return () => { cancelled = true; };
  }, [avatar.id, desktop, folderTree, folderTreeLoadedFor, folderTreeLoading, page, selectedFolderId, snapshotRevision]);

  useEffect(() => {
    setFolderItemsLoadingMore(false);
  }, [avatar.id, page, selectedFolderId]);

  useEffect(() => {
    remoteRequestVersionRef.current += 1;
    remotePageTokenRef.current = null;
    setRemotePageLoading(false);
    if (avatar.id === 'sample-avatar' || page === 'home' || page === 'settings' || page === 'folders' || page === 'empty-folders'
      || (page === 'collections' && !collectionView)) {
      setRemoteLoading(false);
      return;
    }
    if (page === 'collections' && prefsLoadedFor !== avatar.id) return;
    let cancelled = false;
    setRemoteLoading(true);
    setRemoteError('');
    const timer = window.setTimeout(() => {
      const searchKind = kind === 'Any type' ? null : kind;
      const clueFilter = page === 'review' ? candidateFilter : null;
      const collectionItems = page === 'collections' && collectionView
        ? prefs.collections.find((board) => board.id === collectionView)?.itemIds ?? []
        : null;
      const request = collectionItems
        ? invoke<any[]>('get_collection_items', { avatarId: avatar.id, itemIds: collectionItems })
            .then((items) => ({ items, hasMore: false }))
        : invoke<{ items: any[]; hasMore: boolean }>('search_inventory_page', {
            avatarId: avatar.id,
            query,
            kind: searchKind,
            candidateFilter: clueFilter,
            recentOnly: page === 'new',
            sort: sortMode,
            offset: 0,
            limit: INVENTORY_PAGE_SIZE,
          });
      request
        .then((result) => {
          if (cancelled) return;
          setRemoteData({ avatarId: avatar.id, query, kind: searchKind, page, candidateFilter, collectionView, sortMode, snapshotRevision, items: result.items.map(fromLocalIndex), hasMore: result.hasMore });
          setRemoteLoading(false);
        })
        .catch((error) => { if (!cancelled) { setRemoteData((current) => current?.avatarId === avatar.id ? null : current); setRemoteError(String(error)); setRemoteLoading(false); } });
    }, 140);
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [avatar.id, candidateFilter, collectionView, desktop, kind, page, prefs.collections, prefsLoadedFor, query, sortMode, snapshotRevision, tagSearchRevision]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const dialogOpen = helpOpen || saveSearchOpen || collectionOpen || newCollectionDialogOpen || moveDialogOpen || mobileDetailOpen || Boolean(itemContextMenu);
      if (cacheDialogOpen || profileRestore || updateInstalling) return;
      if (!dialogOpen && ((event.key === '/' && !isTyping(event.target)) || ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k'))) {
        event.preventDefault();
        if (page === 'empty-folders' || page === 'home') {
          setPage('find');
          window.requestAnimationFrame(() => searchRef.current?.focus());
        } else {
          (page === 'folders' ? document.querySelector<HTMLInputElement>('.folder-search input') : searchRef.current)?.focus();
        }
      } else if (event.key === 'Escape') {
        if (itemContextMenu) {
          setItemContextMenu(null);
          window.requestAnimationFrame(() => itemContextMenuTriggerRef.current?.focus());
        }
        else if (helpOpen) { setHelpOpen(false); window.requestAnimationFrame(() => helpTriggerRef.current?.focus()); }
        else if (saveSearchOpen) setSaveSearchOpen(false);
        else if (newCollectionDialogOpen) setNewCollectionDialogOpen(false);
        else if (collectionOpen) setCollectionOpen(false);
        else if (moveDialogOpen) setMoveDialogOpen(false);
        else if (mobileDetailOpen) closeMobileDetail();
        else if (sortMenu) setSortMenu(false);
        else if (kindMenu) setKindMenu(false);
        else if (avatarMenu) { setAvatarMenu(false); setAvatarLabelEditorOpen(false); }
        else if (tagEditor) setTagEditor(false);
        else if (page === 'folders' && document.activeElement === document.querySelector('.folder-search input')) {
          setFolderSearch('');
          (document.activeElement as HTMLElement).blur();
        } else if (document.activeElement === searchRef.current) {
          setQuery('');
          searchRef.current?.blur();
        }
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [avatarMenu, cacheDialogOpen, collectionOpen, helpOpen, itemContextMenu, kindMenu, mobileDetailOpen, moveDialogOpen, newCollectionDialogOpen, page, profileRestore, saveSearchOpen, sortMenu, tagEditor, updateInstalling]);

  useEffect(() => {
    if (!itemContextMenu) return;
    const dismissIfOutside = (event: PointerEvent) => {
      if (event.target instanceof Node && itemContextMenuRef.current?.contains(event.target)) return;
      setItemContextMenu(null);
    };
    const dismiss = () => setItemContextMenu(null);
    document.addEventListener('pointerdown', dismissIfOutside, true);
    document.addEventListener('scroll', dismiss, true);
    window.addEventListener('resize', dismiss);
    return () => {
      document.removeEventListener('pointerdown', dismissIfOutside, true);
      document.removeEventListener('scroll', dismiss, true);
      window.removeEventListener('resize', dismiss);
    };
  }, [itemContextMenu]);

  useEffect(() => {
    if (!itemContextMenu) return;
    const frame = window.requestAnimationFrame(() => {
      itemContextMenuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')?.focus({ preventScroll: true });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [itemContextMenu]);

  const requestedKind = kind === 'Any type' ? null : kind;
  const remoteMatchesView = Boolean(remoteData
    && remoteData.avatarId === avatar.id
    && remoteData.query === query
    && remoteData.kind === requestedKind
    && remoteData.page === page
    && remoteData.candidateFilter === candidateFilter
    && remoteData.collectionView === collectionView
    && remoteData.sortMode === sortMode
    && remoteData.snapshotRevision === snapshotRevision);
  const catalog = avatar.id === 'sample-avatar' ? sampleItems : remoteMatchesView && remoteData ? remoteData.items : EMPTY_ITEMS;
  const remoteViewLoading = avatar.id !== 'sample-avatar' && remoteLoading && !remoteMatchesView;
  const effectiveCatalog = useMemo(() => catalog.map((item) => {
    const liveLocation = liveLocationOverrides[`${item.avatarId}\u0000${item.id}`];
    return {
      ...item,
      ...(liveLocation ? { parentFolderId: liveLocation.item.parentFolderId, path: liveLocation.item.path, liveLocationConfirmedAt: liveLocation.confirmedAt } : {}),
      tags: prefs.itemTags[item.id] ?? item.tags,
    };
  }), [catalog, liveLocationOverrides, prefs.itemTags]);

  const visibleItems = useMemo(() => {
    let items = avatar.id === 'sample-avatar' || (page === 'collections' && collectionView)
      ? searchItems(effectiveCatalog, query, { kind }) .map((hit) => hit.item)
      : effectiveCatalog;
    if (page === 'new' && avatar.id === 'sample-avatar') {
      const now = Date.now();
      const cutoff = now - 30 * 86_400_000;
      items = items.filter((item) => {
        const date = new Date(item.createdAt).getTime();
        return date >= cutoff && date <= now;
      });
    }
    if (page === 'review' && avatar.id === 'sample-avatar') items = items.filter((item) => {
      const clue = candidateReason(item);
      return clue && (candidateFilter === 'All clues' || clue.label === candidateFilter);
    });
    if (page === 'collections' && collectionView) {
      const board = prefs.collections.find((item) => item.id === collectionView);
      const itemOrder = new Map((board?.itemIds ?? []).map((id, index) => [id, index]));
      items = items
        .filter((item) => itemOrder.has(item.id))
        .sort((left, right) => itemOrder.get(left.id)! - itemOrder.get(right.id)!);
    }
    if (avatar.id === 'sample-avatar' && !(page === 'collections' && collectionView)) {
      const activeSort = sortMode;
      if (activeSort === 'Name A–Z') items = [...items].sort((a, b) => a.name.localeCompare(b.name));
      if (activeSort === 'Newest first') items = [...items].sort((a, b) => {
        const aTime = new Date(a.createdAt).getTime();
        const bTime = new Date(b.createdAt).getTime();
        return (Number.isNaN(bTime) ? -Infinity : bTime) - (Number.isNaN(aTime) ? -Infinity : aTime) || a.name.localeCompare(b.name);
      });
    }
    return items;
  }, [avatar.id, candidateFilter, collectionView, effectiveCatalog, kind, page, prefs.collections, query, sortMode]);

  const folderCatalog = useMemo(
    () => avatar.id === 'sample-avatar'
      ? buildSampleFolders(sampleItems)
      : folderTreeLoadedFor === avatar.id ? folderTree : EMPTY_FOLDERS,
    [avatar.id, folderTree, folderTreeLoadedFor],
  );
  const folderIndex = useMemo(() => createFolderIndex(folderCatalog), [folderCatalog]);
  const currentFolderTrail = useMemo(() => folderIndex.getTrail(selectedFolderId), [folderIndex, selectedFolderId]);
  const folderPathLabels = folderIndex.pathLabels;
  const folderSafety = useMemo(
    () => createFolderProtectionResolver(folderCatalog, prefs.protectedFolderIds, INVENTORY_ROOT_ID),
    [folderCatalog, prefs.protectedFolderIds],
  );
  const childFolders = useMemo(
    () => folderIndex.childrenByParent.get(selectedFolderId ?? INVENTORY_ROOT_ID) ?? EMPTY_FOLDERS,
    [folderIndex, selectedFolderId],
  );
  const folderMatches = useMemo(() => {
    const text = folderSearch.trim().toLocaleLowerCase();
    if (!text) return [];
    return folderIndex.searchEntries.filter(({ searchText }) => searchText.includes(text)).map(({ folder }) => folder);
  }, [folderIndex, folderSearch]);
  const folderViewItems = useMemo(() => {
    const currentParentId = selectedFolderId ?? INVENTORY_ROOT_ID;
    const movedItems = Object.values(liveLocationOverrides)
      .filter((move) => move.avatarId === avatar.id && move.item.parentFolderId === currentParentId)
      .map((move) => move.item);
    const byId = new Map<string, InventoryItem>();
    for (const item of folderItems) {
      const liveLocation = liveLocationOverrides[`${item.avatarId}\u0000${item.id}`];
      if (liveLocation && liveLocation.item.parentFolderId !== currentParentId) continue;
      byId.set(item.id, liveLocation?.item ?? item);
    }
    for (const item of movedItems) byId.set(item.id, item);
    return [...byId.values()].map((item) => ({ ...item, tags: prefs.itemTags[item.id] ?? item.tags }));
  }, [avatar.id, folderItems, liveLocationOverrides, prefs.itemTags, selectedFolderId]);
  const folderDisplayedTotal = useMemo(() => {
    const parentId = selectedFolderId ?? INVENTORY_ROOT_ID;
    const change = Object.values(liveLocationOverrides).reduce((total, move) => {
      if (move.avatarId !== avatar.id || move.previousParentFolderId === move.item.parentFolderId) return total;
      return total + Number(move.item.parentFolderId === parentId) - Number(move.previousParentFolderId === parentId);
    }, 0);
    return Math.max(0, folderItemsTotal + change);
  }, [avatar.id, folderItemsTotal, liveLocationOverrides, selectedFolderId]);
  const selected = page === 'empty-folders' ? null : page === 'folders'
    ? folderViewItems.find((item) => item.id === selectedId) ?? folderViewItems[0] ?? null
    : visibleItems.find((item) => item.id === selectedId) ?? visibleItems[0] ?? null;
  useEffect(() => { setTagEditor(false); setTagDraft(''); setCollectionOpen(false); }, [avatar.id, selected?.id]);
  activeItemPhotoKeyRef.current = selected ? itemImageKey(avatar.id, selected.id) : null;
  const selectedImageUrl = selectedImage?.key === activeItemPhotoKeyRef.current ? selectedImage.url : null;
  const selectedThumbnailUrl = selectedThumbnail?.key === activeItemPhotoKeyRef.current ? selectedThumbnail.url : null;
  const protectedItemIds = useMemo(() => new Set(prefs.protectedItemIds), [prefs.protectedItemIds]);
  const itemSafetyById = useMemo(() => new Map(visibleItems.map((item) => [item.id, classifyItemSafety(item, protectedItemIds, folderSafety)])), [visibleItems, protectedItemIds, folderSafety]);
  const selectedSafety = useMemo(
    () => selected ? classifyItemSafety(selected, protectedItemIds, folderSafety) : null,
    [folderSafety, protectedItemIds, selected],
  );
  const selectedProtectedDirectly = selectedSafety?.isItemMarked ?? false;
  const selectedProtectedByFolder = Boolean(selectedSafety?.reasons.some((reason) => reason.source === 'folder-mark'));
  const selectedProtected = selectedSafety?.isProtected ?? false;
  const selectedSafetyCopy = selectedSafety?.reasons.map((reason) => reason.label).join(' · ') ?? '';
  const stockLeapAvatarKey = stockLeapStatus.avatarId ? `${stockLeapStatus.avatarId}@secondlife` : null;
  const stockLeapAvatarLabel = stockLeapAvatarKey ? firestormAvatarNames[stockLeapAvatarKey] ?? avatarLabels[stockLeapAvatarKey] ?? avatarLabels[stockLeapStatus.avatarId ?? ''] ?? null : null;
  const moveBridgeSupported = supportsStockLeapMove(avatar.id);
  const ordinaryFolderMovesPaused = !STOCK_LEAP_ORDINARY_FOLDER_MOVES_ENABLED;
  const stockLeapMoveReady = stockLeapStatus.connected && stockLeapStatus.avatarId?.toLowerCase() === avatar.id.split('@')[0].toLowerCase()
    && ['LLAgent', 'LLCommandDispatcher', 'LLInventory', 'LLWindow'].every((api) => stockLeapStatus.apiNames.includes(api));
  const moveDestinationOptions = useMemo(() => {
    if (!moveDialogOpen || !selected) return [] as { folder: FolderSummary; path: string }[];
    const filter = moveFolderSearch.trim().toLocaleLowerCase();
    return folderCatalog
      .filter((folder) => folder.id !== selected.parentFolderId)
      .map((folder) => ({ folder, path: folderPathLabels.get(folder.id) || folder.name }))
      .filter(({ path }) => !filter || path.toLocaleLowerCase().includes(filter))
      .sort((a, b) => a.path.localeCompare(b.path));
  }, [folderCatalog, folderPathLabels, moveDialogOpen, moveFolderSearch, selected]);
  useEffect(() => {
    let cancelled = false;
    setLinkTarget(null);
    setLinkTargetError(false);
    if (!desktop || avatar.id === 'sample-avatar' || !selected?.isLink || !selected.linkedTargetId) {
      setLinkTargetLoading(false);
      return () => { cancelled = true; };
    }
    setLinkTargetLoading(true);
    invoke<InventoryLinkTarget | null>('get_inventory_link_target', {
      avatarId: avatar.id,
      targetId: selected.linkedTargetId,
    })
      .then((target) => { if (!cancelled) setLinkTarget(target); })
      .catch(() => { if (!cancelled) setLinkTargetError(true); })
      .finally(() => { if (!cancelled) setLinkTargetLoading(false); });
    return () => { cancelled = true; };
  }, [avatar.id, desktop, selected?.id, selected?.isLink, selected?.linkedTargetId, snapshotRevision]);
  useEffect(() => {
    let cancelled = false;
    let objectUrl: string | null = null;
    const version = ++photoRequestVersionRef.current;
    setSelectedImage(null);
    if (!selected) return () => undefined;
    void readLocalImage(itemImageKey(avatar.id, selected.id)).then((image) => {
      if (cancelled || version !== photoRequestVersionRef.current || !image) return;
      objectUrl = URL.createObjectURL(image);
      setSelectedImage({ key: itemImageKey(avatar.id, selected.id), url: objectUrl });
    }).catch(() => undefined);
    return () => {
      cancelled = true;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [avatar.id, localMediaRevision, selected?.id]);
  useEffect(() => {
    let cancelled = false;
    setSelectedThumbnail(null);
    if (!desktop || avatar.id === 'sample-avatar' || !selected || selectedImageUrl) {
      return () => { cancelled = true; };
    }
    const timer = window.setTimeout(() => {
      void invoke<string | null>('get_inventory_thumbnail_preview', {
        avatarId: avatar.id,
        itemId: selected.id,
      })
        .then((preview) => { if (!cancelled && preview) setSelectedThumbnail({ key: itemImageKey(avatar.id, selected.id), url: preview }); })
        .catch(() => undefined);
    }, 150);
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [avatar.id, desktop, selected?.id, selectedImageUrl, snapshotRevision]);
  const clueCounts = avatar.id === 'sample-avatar'
    ? CANDIDATE_FILTERS.reduce((counts, filter) => {
      counts[filter] = filter === 'All clues'
        ? effectiveCatalog.filter((item) => candidateReason(item)).length
        : effectiveCatalog.filter((item) => candidateReason(item)?.label === filter).length;
      return counts;
    }, {} as Record<CandidateFilter, number>)
    : {
      'All clues': avatar.candidateCounts?.allClues ?? 0,
      'Demo-like name': avatar.candidateCounts?.demoLikeName ?? 0,
      'Possible package': avatar.candidateCounts?.possiblePackage ?? 0,
      'Possible body-fit variant': avatar.candidateCounts?.bodyFitVariant ?? 0,
      'Possible duplicate': avatar.candidateCounts?.possibleDuplicate ?? 0,
      Notecard: avatar.candidateCounts?.notecard ?? 0,
      Landmark: avatar.candidateCounts?.landmark ?? 0,
    };
  const candidateCount = clueCounts['All clues'];
  const recentCount = avatar.id === 'sample-avatar'
    ? effectiveCatalog.filter((item) => Date.now() - new Date(item.createdAt).getTime() <= 30 * 86_400_000 && new Date(item.createdAt).getTime() <= Date.now()).length
    : avatar.recentCount ?? 0;

  const pushToast = useCallback((message: string) => {
    if (toastTimerRef.current !== null) window.clearTimeout(toastTimerRef.current);
    setToast(message);
    toastTimerRef.current = window.setTimeout(() => { setToast(''); toastTimerRef.current = null; }, 3500);
  }, []);

  useEffect(() => () => {
    if (toastTimerRef.current !== null) window.clearTimeout(toastTimerRef.current);
  }, []);

  async function importCache() {
    if (!desktop) {
      pushToast('Cache import is available in the desktop app. This browser preview uses sample data.');
      return;
    }
    void openCacheChooser();
  }

  async function openCacheChooser() {
    if (cacheChooserActiveRef.current || importBusyRef.current) return;
    cacheChooserActiveRef.current = true;
    selectedCacheCandidateRef.current = null;
    setSelectedCacheCandidate(null);
    setCacheDialogOpen(true);
    await refreshCacheDiscovery();
  }

  async function refreshCacheDiscovery() {
    if (!cacheChooserActiveRef.current || importBusyRef.current) return;
    const version = ++cacheRequestVersionRef.current;
    setCacheDialogError('');
    setFirestormRunning(null);
    setFirestormCacheDirectories([]);
    setCacheDiscoveryBusy(true);
    try {
      const found = await invoke<FirestormDiscovery>('discover_firestorm_caches');
      if (version !== cacheRequestVersionRef.current || !cacheChooserActiveRef.current) return;
      const selectedCandidate = selectedCacheCandidateRef.current;
      const candidates = selectedCandidate && !found.candidates.some((candidate) => candidate.path === selectedCandidate.path)
        ? [selectedCandidate, ...found.candidates]
        : found.candidates;
      setCacheCandidates(candidates);
      setFirestormAvatarNames((current) => ({ ...current, ...found.avatarNames }));
      setAvatars((current) => current.map((option) => ({
        ...option,
        label: avatarLabelsRef.current[option.id] ?? found.avatarNames[option.id] ?? option.label,
      })));
      setAvatar((current) => current.id === 'sample-avatar' || avatarLabelsRef.current[current.id]
        ? current
        : { ...current, label: found.avatarNames[current.id] ?? current.label });
      setFirestormRunning(found.viewerRunning);
      setFirestormCacheDirectories(found.cacheDirectories);
    } catch (error) {
      if (version === cacheRequestVersionRef.current && cacheChooserActiveRef.current) setCacheDialogError(`Could not check Firestorm cache folders: ${String(error)}`);
    } finally {
      if (version === cacheRequestVersionRef.current) setCacheDiscoveryBusy(false);
    }
  }

  function closeCacheChooser() {
    if (importBusyRef.current) return;
    cacheChooserActiveRef.current = false;
    cacheRequestVersionRef.current += 1;
    setCacheDiscoveryBusy(false);
    setCacheDialogOpen(false);
  }

  async function importCachePath(path: string, avatarLabel = '', options: { preserveView?: boolean } = {}): Promise<boolean> {
    if (importBusyRef.current) return false;
    if (profileBusyRef.current || tagSaveBusyRef.current) {
      pushToast('Wait for the profile or tag save to finish before importing a snapshot.');
      return false;
    }
    if (moveBusyRef.current) { pushToast('Wait for Firestorm to finish the current move before importing a snapshot.'); return false; }
    cacheRequestVersionRef.current += 1;
    setCacheDiscoveryBusy(false);
    importBusyRef.current = true;
    setBusy(true);
    setImportProgress({ stage: 'reading_cache', completed: 0, total: null });
    setImportCancellationPending(false);
    setCacheDialogError('');
    try {
      const result = await invoke<ImportSummary>('import_inventory_cache', { path });
      avatarListRequestVersionRef.current += 1;
      setAvatarListLoaded(true);
      setAvatarListError('');
      let tagsRestored = false;
      let storedProfile: string | null = null;
      try { storedProfile = localStorage.getItem(`shelfmark:${result.avatarId}`); } catch { /* A saved index remains usable if preference storage is unavailable. */ }
      const localPreferences = unsavedPreferencesRef.current.get(result.avatarId) ?? readPrefs(result.avatarId);
      const hasLocalPreferences = Boolean(storedProfile || unsavedPreferencesRef.current.has(result.avatarId));
      if (desktop && hasLocalPreferences) {
        try {
          tagsRestored = await invoke<boolean>('replace_avatar_tags', { avatarId: result.avatarId, itemTags: localPreferences.itemTags });
          if (tagsRestored) setTagSyncErrors((current) => Object.fromEntries(Object.entries(current).filter(([key]) => !key.startsWith(`${result.avatarId}\u0000`))));
        } catch {
          tagsRestored = false;
        }
      }
      const labels = saveAvatarLabel(result.avatarId, avatarLabel);
      setAvatarLabels(labels);
      const option: AvatarOption = {
        id: result.avatarId,
        label: labels[result.avatarId] ?? firestormAvatarNames[result.avatarId] ?? shortId(result.avatarId),
        itemCount: result.itemCount,
        categoryCount: result.categoryCount,
        cacheVersion: result.cacheVersion,
        snapshotAt: result.snapshotAt,
        recentCount: result.recentCount,
        emptyFolderCount: result.emptyFolderCount,
        candidateCounts: result.candidateCounts,
        inventoryCachePath: result.inventoryCachePath,
        cacheModifiedAtMs: result.cacheModifiedAtMs,
      };
      setAvatars((current) => [option, ...current.filter((entry) => entry.id !== option.id)]);
      setRemoteData(null);
      setLiveLocationOverrides((current) => Object.fromEntries(Object.entries(current).filter(([, move]) => move.avatarId !== result.avatarId)));
      setSnapshotRefreshNeededFor((current) => current === result.avatarId ? null : current);
      setRemoteError('');
      setAvatar(option);
      setSnapshotRevision((revision) => revision + 1);
      if (!options.preserveView) {
        setSelectedFolderId(null);
        setFolderSearch('');
      }
      setImportSummary(result);
      if (!options.preserveView) {
        setPage('find');
        setQuery('');
      }
      setCacheDialogOpen(false);
      cacheChooserActiveRef.current = false;
      pushToast(`Local snapshot indexed · ${result.itemCount.toLocaleString()} items${hasLocalPreferences && !tagsRestored ? ' · personal tags need a refresh' : ''}`);
      return true;
    } catch (error) {
      setCacheDialogError(String(error));
      if (!cacheDialogOpen) pushToast(`Snapshot refresh failed: ${String(error)}`);
      return false;
    } finally {
      importBusyRef.current = false;
      setBusy(false);
      setImportProgress(null);
      setImportCancellationPending(false);
    }
  }

  async function cancelInventoryImport() {
    if (!importBusyRef.current || importCancellationPending) return;
    setImportCancellationPending(true);
    try {
      const cancelled = await invoke<boolean>('cancel_inventory_import');
      if (!cancelled) {
        setImportCancellationPending(false);
        pushToast('The import has finished. Shelfmark is completing the local setup.');
      }
    } catch (error) {
      setCacheDialogError(`Could not stop the import: ${String(error)}`);
      setImportCancellationPending(false);
    }
  }

  async function selectInFirestorm(inventoryId: string, entryType: 'item' | 'folder' = 'item') {
    if (viewerActionBusyRef.current) return;
    if (!desktop || avatar.id === 'sample-avatar' || !isInventoryUuid(inventoryId)) {
      pushToast('Import a Firestorm snapshot in the desktop app to use viewer selection.');
      return;
    }
    viewerActionBusyRef.current = true;
    setViewerActionBusy(true);
    try {
      await invoke('open_inventory_entry_in_firestorm', { avatarId: avatar.id, inventoryId });
      pushToast(`Opening this ${entryType} in your registered Second Life viewer. Check its active avatar and grid.`);
    } catch (error) {
      pushToast(String(error));
    } finally {
      viewerActionBusyRef.current = false;
      setViewerActionBusy(false);
    }
  }

  async function openMoveDialog(targetItem: InventoryItem | null = selected) {
    if (!desktop || !targetItem || avatar.id === 'sample-avatar' || !moveBridgeSupported || !stockLeapMoveReady || !targetItem.parentFolderId) {
      pushToast('Launch Firestorm from Home, select the matching main-grid snapshot, and confirm Firestorm is on the Second Life main grid before moving an item.');
      return;
    }
    const requestToken = ++moveDialogLoadTokenRef.current;
    const requestedAvatarId = avatar.id;
    setSelectedId(targetItem.id);
    setTagEditor(false);
    setMoveDialogLoading(true);
    try {
      if (folderTreeLoadedFor !== avatar.id) {
        const folders = await invoke<FolderSummary[]>('list_inventory_folders', { avatarId: requestedAvatarId });
        if (moveDialogLoadTokenRef.current !== requestToken || activeAvatarIdRef.current !== requestedAvatarId) return;
        setFolderTree(folders);
        setFolderTreeLoadedFor(requestedAvatarId);
      }
      if (moveDialogLoadTokenRef.current !== requestToken || activeAvatarIdRef.current !== requestedAvatarId) return;
      setMoveFolderSearch('');
      setMoveDestinationId('');
      setMoveDialogOpen(true);
    } catch (error) {
      if (moveDialogLoadTokenRef.current === requestToken && activeAvatarIdRef.current === requestedAvatarId) pushToast(String(error));
    } finally {
      if (moveDialogLoadTokenRef.current === requestToken) setMoveDialogLoading(false);
    }
  }

  async function requestItemMove(destinationFolderId: string | null, targetItem: InventoryItem | null = selected) {
    if (!desktop || !targetItem || avatar.id === 'sample-avatar' || !moveBridgeSupported || !stockLeapMoveReady || !targetItem.parentFolderId || moveBusyRef.current || importBusyRef.current) return;
    moveBusyRef.current = true;
    const requestedAvatarId = avatar.id;
    const requestedPage = page;
    const requestedFolderId = selectedFolderId;
    const movingItem = targetItem;
    setSelectedId(movingItem.id);
    setMoveFeedback({ itemId: movingItem.id, state: 'pending', message: 'Asking Firestorm to move this item…' });
    setMoveBusy(true);
    try {
      let folders = folderTreeLoadedFor === requestedAvatarId ? folderTree : [];
      if (!folders.length) {
        try {
          folders = await invoke<FolderSummary[]>('list_inventory_folders', { avatarId: requestedAvatarId });
          if (activeAvatarIdRef.current === requestedAvatarId) {
            setFolderTree(folders);
            setFolderTreeLoadedFor(requestedAvatarId);
          }
        } catch { /* A confirmed viewer move can still be shown with its destination name. */ }
      }
      const result = await invoke<StockLeapMoveResult>('move_stock_firestorm_inventory_item', {
        avatarId: requestedAvatarId,
        itemId: movingItem.id,
        destinationFolderId,
      });
      if (result.changed) {
        const destinationTrail = folderTrail(result.item.parentId, folders).map((folder) => folder.name);
        const nextItem: InventoryItem = {
          ...movingItem,
          parentFolderId: result.item.parentId,
          path: destinationTrail.length ? destinationTrail : result.item.parentId === INVENTORY_ROOT_ID ? [] : [result.destinationName],
          liveLocationConfirmedAt: new Date().toISOString(),
        };
        const previousParentFolderId = result.previousParentId;
        const key = `${requestedAvatarId}\u0000${movingItem.id}`;
        setLiveLocationOverrides((current) => ({ ...current, [key]: { avatarId: requestedAvatarId, item: nextItem, previousParentFolderId: current[key]?.previousParentFolderId ?? previousParentFolderId, confirmedAt: nextItem.liveLocationConfirmedAt! } }));
        const stillInRequestedFolder = activeAvatarIdRef.current === requestedAvatarId
          && activePageRef.current === requestedPage
          && activeSelectedFolderIdRef.current === requestedFolderId;
        if (stillInRequestedFolder && requestedPage === 'folders') {
          const currentFolderId = requestedFolderId ?? INVENTORY_ROOT_ID;
          const leftCurrentFolder = previousParentFolderId === currentFolderId && previousParentFolderId !== result.item.parentId;
          setFolderItems((current) => {
            const withoutMovedItem = current.filter((item) => item.id !== movingItem.id);
            return result.item.parentId === currentFolderId ? [nextItem, ...withoutMovedItem] : withoutMovedItem;
          });
          if (leftCurrentFolder) {
            const nextSelectedId = folderViewItems.find((item) => item.id !== movingItem.id)?.id ?? '';
            if (activeSelectedIdRef.current === movingItem.id) setSelectedId(nextSelectedId);
          }
        }
      }
      if (activeAvatarIdRef.current === requestedAvatarId) {
        setMoveFeedback({
          itemId: movingItem.id,
          state: 'success',
          message: result.changed
            ? `Moved to ${result.destinationName}. Firestorm confirmed the new folder; refresh the snapshot after closing Firestorm to sync its saved cache.`
            : `Already in ${result.destinationName}; no move was needed.`,
        });
        setMoveDialogOpen(false);
      }
      if (result.changed) setSnapshotRefreshNeededFor(requestedAvatarId);
      pushToast(result.changed ? 'Move confirmed by Firestorm.' : 'Firestorm confirmed the item was already there.');
    } catch (error) {
      if (activeAvatarIdRef.current === requestedAvatarId) setMoveFeedback({ itemId: movingItem.id, state: 'error', message: String(error) });
    } finally {
      moveBusyRef.current = false;
      setMoveBusy(false);
    }
  }

  const selectInventoryItem = useCallback((item: InventoryItem) => {
    setSelectedId(item.id);
    setTagEditor(false);
    if (window.matchMedia('(max-width: 900px)').matches) setMobileDetailOpen(true);
  }, []);

  const openItemContextMenu = useCallback((item: InventoryItem, event: ReactMouseEvent<HTMLButtonElement>) => {
    if (!desktop) return;
    event.preventDefault();
    itemContextMenuTriggerRef.current = event.currentTarget;
    setSelectedId(item.id);
    setTagEditor(false);
    setCollectionOpen(false);
    setItemContextMenu({ item, x: event.clientX, y: event.clientY });
  }, [desktop]);

  function handleItemContextMenuKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    const menuItems = Array.from(itemContextMenuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)') ?? []);
    if (!menuItems.length) return;
    const currentIndex = menuItems.indexOf(document.activeElement as HTMLButtonElement);
    let nextIndex: number | null = null;
    if (event.key === 'ArrowDown') nextIndex = (currentIndex + 1) % menuItems.length;
    else if (event.key === 'ArrowUp') nextIndex = (currentIndex - 1 + menuItems.length) % menuItems.length;
    else if (event.key === 'Home') nextIndex = 0;
    else if (event.key === 'End') nextIndex = menuItems.length - 1;
    if (nextIndex !== null) {
      event.preventDefault();
      menuItems[nextIndex].focus();
    }
  }

  async function refreshAvatarSnapshot() {
    if (!desktop || avatar.id === 'sample-avatar' || snapshotRefreshBusyRef.current || importBusyRef.current) return;
    snapshotRefreshBusyRef.current = true;
    if (!avatar.inventoryCachePath) {
      setSnapshotRefreshBusy(true);
      try {
        const state = await invoke<FirestormDiscovery>('discover_firestorm_caches');
        setFirestormRunning(state.viewerRunning);
        if (state.viewerRunning === true) {
          pushToast('Close all Firestorm windows first. The viewer saves this inventory cache when it disconnects.');
          return;
        }
        if (state.viewerRunning !== false) {
          pushToast('Shelfmark could not confirm Firestorm is closed. Close the viewer, then try again.');
          return;
        }
        pushToast('This older snapshot has no saved cache path. Choose its cache once to enable direct refresh next time.');
        await openCacheChooser();
      } catch (error) {
        pushToast(`Could not prepare snapshot refresh: ${String(error)}`);
      } finally {
        snapshotRefreshBusyRef.current = false;
        setSnapshotRefreshBusy(false);
      }
      return;
    }
    setSnapshotRefreshBusy(true);
    try {
      const state = await invoke<FirestormDiscovery>('discover_firestorm_caches');
      setFirestormRunning(state.viewerRunning);
      if (state.viewerRunning === true) {
        pushToast('Close all Firestorm windows first. The viewer saves this inventory cache when it disconnects.');
        return;
      }
      if (state.viewerRunning !== false) {
        pushToast('Shelfmark could not confirm Firestorm is closed. Close the viewer, then try again.');
        return;
      }
      const refreshed = await importCachePath(avatar.inventoryCachePath, avatar.label, { preserveView: true });
      if (refreshed) {
        setSnapshotRefreshNeededFor((pending) => pending === avatar.id ? null : pending);
        pushToast('Snapshot refreshed from the selected Firestorm cache.');
      }
    } catch (error) {
      pushToast(`Could not refresh this snapshot: ${String(error)}`);
    } finally {
      snapshotRefreshBusyRef.current = false;
      setSnapshotRefreshBusy(false);
    }
  }

  async function exportProfile() {
    if (profileBusyRef.current) return;
    if (photoBusyRef.current) { pushToast('Wait for the photo to finish saving, then export your profile.'); return; }
    if (importBusyRef.current || tagSaveBusyRef.current) { pushToast('Wait for the inventory import or tag save to finish before exporting.'); return; }
    profileBusyRef.current = true;
    setProfileBusy(true);
    try {
      const overrides = Object.fromEntries([...unsavedPreferencesRef.current].map(([id, value]) => [`shelfmark:${id}`, JSON.stringify(value)]));
      if (prefsLoadedFor === avatar.id) overrides[`shelfmark:${avatar.id}`] = JSON.stringify(prefs);
      const storage = withProfileStorageOverrides(localStorage, overrides);
      const photos = await listLocalImages();
      const contents = await createProfileBackup(storage, photos);
      const fileName = `Shelfmark-profile-${new Date().toISOString().slice(0, 10)}.shelfmark.json`;
      if (desktop) {
        const path = await save({
          title: 'Save your Shelfmark profile',
          defaultPath: fileName,
          filters: [{ name: 'Shelfmark profile', extensions: ['json'] }],
        });
        if (!path) return;
        await writeTextFile(path, contents);
      } else {
        const url = URL.createObjectURL(new Blob([contents], { type: 'application/json;charset=utf-8' }));
        const link = document.createElement('a');
        link.href = url;
        link.download = fileName;
        link.click();
        window.setTimeout(() => URL.revokeObjectURL(url), 1_000);
      }
      pushToast('Profile saved locally · inventory snapshots are not included.');
    } catch (error) {
      pushToast(error instanceof Error ? error.message : 'Could not export your Shelfmark profile.');
    } finally {
      profileBusyRef.current = false;
      setProfileBusy(false);
    }
  }

  async function readProfileFile(event: React.ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0];
    event.target.value = '';
    if (!file) return;
    if (profileBusyRef.current || updateInstalling) return;
    if (file.size > MAX_PROFILE_FILE_CHARS) {
      pushToast('That profile file is larger than the supported 128 MB limit.');
      return;
    }
    profileBusyRef.current = true;
    setProfileBusy(true);
    try {
      const backup = parseProfileBackup(await file.text());
      setProfileRestore(backup);
    } catch (error) {
      pushToast(error instanceof Error ? error.message : 'Could not read that Shelfmark profile file.');
    } finally {
      profileBusyRef.current = false;
      setProfileBusy(false);
    }
  }

  async function applyProfileRestore() {
    const backup = profileRestore;
    if (!backup || profileBusyRef.current) return;
    if (photoBusyRef.current) { pushToast('Wait for the photo to finish saving before restoring a profile.'); return; }
    if (importBusyRef.current || tagSaveBusyRef.current) { pushToast('Wait for the inventory import or tag save to finish before restoring.'); return; }
    profileBusyRef.current = true;
    profileRestoreBusyRef.current = true;
    setProfileBusy(true);
    try {
      const oldStorage = collectProfileStorage(localStorage);
      const oldImages = await listLocalImages();
      const restoredImages = backup.images.map((image) => ({ key: image.key, image: decodeProfileImage(image) }));
      replaceProfileStorage(localStorage, backup.localStorage);
      try {
        await replaceLocalImages(restoredImages);
        if (desktop) await invoke('replace_profile_tags', { profiles: avatarTagsFromBackup(backup) });
      } catch (error) {
        const recoveryFailures: string[] = [];
        try { replaceProfileStorage(localStorage, oldStorage); }
        catch { recoveryFailures.push('personal settings'); }
        try { await replaceLocalImages(oldImages); }
        catch { recoveryFailures.push('local photos'); }
        if (recoveryFailures.length) throw new Error(`Profile restore failed, and Shelfmark could not fully recover ${recoveryFailures.join(' and ')}. Keep this window open and use your previous profile backup to recover.`);
        throw error;
      }
      setProfileRestore(null);
      window.location.reload();
    } catch (error) {
      pushToast(error instanceof Error ? error.message : 'Could not restore that Shelfmark profile.');
    } finally {
      profileRestoreBusyRef.current = false;
      profileBusyRef.current = false;
      setProfileBusy(false);
    }
  }

  function saveCurrentAvatarLabel() {
    if (avatar.id === 'sample-avatar') return;
    const labels = saveAvatarLabel(avatar.id, avatarLabelDraft);
    const label = labels[avatar.id] ?? firestormAvatarNames[avatar.id] ?? shortId(avatar.id);
    setAvatarLabels(labels);
    setAvatar((current) => current.id === avatar.id ? { ...current, label } : current);
    setAvatars((current) => current.map((entry) => entry.id === avatar.id ? { ...entry, label } : entry));
    setAvatarLabelEditorOpen(false);
    pushToast(labels[avatar.id] ? 'Avatar display name saved on this device.' : 'Avatar label cleared.');
  }

  async function loadMoreInventory() {
    if (!desktop || !remoteMatchesView || !remoteData?.hasMore || remoteLoading || remotePageTokenRef.current) return;
    const pageToken = {};
    remotePageTokenRef.current = pageToken;
    setRemotePageLoading(true);
    setRemoteError('');
    const requestVersion = remoteRequestVersionRef.current;
    try {
      const result = await invoke<{ items: any[]; hasMore: boolean }>('search_inventory_page', {
        avatarId: avatar.id,
        query,
        kind: kind === 'Any type' ? null : kind,
        candidateFilter: page === 'review' ? candidateFilter : null,
        recentOnly: page === 'new',
        sort: sortMode,
        offset: remoteData.items.length,
        limit: INVENTORY_PAGE_SIZE,
      });
      if (requestVersion !== remoteRequestVersionRef.current) return;
      const nextItems = result.items.map(fromLocalIndex);
      setRemoteData((current) => {
        if (!current
          || current.avatarId !== avatar.id
          || current.query !== query
          || current.kind !== (kind === 'Any type' ? null : kind)
          || current.page !== page
          || current.candidateFilter !== candidateFilter
          || current.collectionView !== collectionView
          || current.sortMode !== sortMode
          || current.snapshotRevision !== snapshotRevision) return current;
        const existing = new Set(current.items.map((item) => item.id));
        return { ...current, items: [...current.items, ...nextItems.filter((item) => !existing.has(item.id))], hasMore: result.hasMore };
      });
    } catch (error) {
      if (requestVersion === remoteRequestVersionRef.current) setRemoteError(String(error));
    } finally {
      if (remotePageTokenRef.current === pageToken) {
        remotePageTokenRef.current = null;
        setRemotePageLoading(false);
      }
    }
  }

  async function loadMoreFolderItems() {
    if (!desktop || page !== 'folders' || folderTreeLoading || folderTreeLoadedFor !== avatar.id || folderItemsLoading || !folderItemsHasMore || folderPageTokenRef.current) return;
    const pageToken = {};
    folderPageTokenRef.current = pageToken;
    const requestedAvatarId = avatar.id;
    const requestedPage = page;
    const requestedFolderId = selectedFolderId;
    const requestedOffset = folderPageOffsetRef.current;
    const requestVersion = folderRequestVersionRef.current;
    setFolderItemsLoadingMore(true);
    setFolderItemsError('');
    try {
      const result = await invoke<FolderContentsResult>('get_inventory_folder_contents', {
        avatarId: requestedAvatarId,
        folderId: requestedFolderId,
        offset: requestedOffset,
        limit: INVENTORY_PAGE_SIZE,
      });
      if (requestVersion !== folderRequestVersionRef.current || activeAvatarIdRef.current !== requestedAvatarId || activePageRef.current !== requestedPage || activeSelectedFolderIdRef.current !== requestedFolderId) return;
      folderPageOffsetRef.current = requestedOffset + result.items.length;
      const nextItems = result.items.map(fromLocalIndex);
      setFolderItems((current) => {
        const existing = new Set(current.map((item) => item.id));
        return [...current, ...nextItems.filter((item) => !existing.has(item.id))];
      });
      setFolderItemsHasMore(result.hasMore);
      setFolderItemsTotal(result.totalItemCount);
    } catch (error) {
      if (requestVersion === folderRequestVersionRef.current) setFolderItemsError(String(error));
    } finally {
      if (folderPageTokenRef.current === pageToken) {
        folderPageTokenRef.current = null;
        setFolderItemsLoadingMore(false);
      }
    }
  }

  async function loadMoreEmptyFolders() {
    if (!desktop || page !== 'empty-folders' || emptyFoldersLoading || !emptyFoldersHasMore || emptyPageTokenRef.current) return;
    const pageToken = {};
    emptyPageTokenRef.current = pageToken;
    const requestedAvatarId = avatar.id;
    const requestVersion = emptyRequestVersionRef.current;
    setEmptyFoldersLoadingMore(true);
    setEmptyFoldersError('');
    try {
      const result = await invoke<EmptyFoldersPageResult>('list_empty_inventory_folders', {
        avatarId: requestedAvatarId,
        offset: emptyFolders.length,
        limit: INVENTORY_PAGE_SIZE,
      });
      if (requestVersion !== emptyRequestVersionRef.current || activeAvatarIdRef.current !== requestedAvatarId) return;
      setEmptyFolders((current) => {
        const existing = new Set(current.map((folder) => folder.id));
        return [...current, ...result.folders.filter((folder) => !existing.has(folder.id))];
      });
      setEmptyFoldersHasMore(result.hasMore);
      setEmptyFoldersTotal(result.totalCount);
    } catch (error) {
      if (requestVersion === emptyRequestVersionRef.current) setEmptyFoldersError(String(error));
    } finally {
      if (emptyPageTokenRef.current === pageToken) {
        emptyPageTokenRef.current = null;
        setEmptyFoldersLoadingMore(false);
      }
    }
  }

  async function retryEmptyFolders() {
    if (avatar.id === 'sample-avatar') return;
    const requestedAvatarId = avatar.id;
    const requestVersion = ++emptyRequestVersionRef.current;
    emptyPageTokenRef.current = null;
    setEmptyFoldersLoadingMore(false);
    setEmptyFoldersLoading(true);
    setEmptyFoldersError('');
    try {
      const result = await invoke<EmptyFoldersPageResult>('list_empty_inventory_folders', {
        avatarId: requestedAvatarId,
        offset: 0,
        limit: INVENTORY_PAGE_SIZE,
      });
      if (requestVersion !== emptyRequestVersionRef.current || activeAvatarIdRef.current !== requestedAvatarId) return;
      setEmptyFolders(result.folders);
      setEmptyFoldersHasMore(result.hasMore);
      setEmptyFoldersTotal(result.totalCount);
    } catch (error) {
      if (requestVersion === emptyRequestVersionRef.current) setEmptyFoldersError(String(error));
    } finally {
      if (requestVersion === emptyRequestVersionRef.current) setEmptyFoldersLoading(false);
    }
  }

  async function browseForCache() {
    if (!cacheChooserActiveRef.current || importBusyRef.current) return;
    const version = ++cacheRequestVersionRef.current;
    setCacheDiscoveryBusy(true);
    setCacheDialogError('');
    try {
      const selectedFile = await open({
        title: 'Browse for a Firestorm inventory cache',
        multiple: false,
        directory: false,
        filters: [{ name: 'Firestorm inventory cache', extensions: ['gz', 'llsd', 'inv'] }],
      });
      if (version !== cacheRequestVersionRef.current || !cacheChooserActiveRef.current) return;
      if (typeof selectedFile === 'string') {
        const candidate = await invoke<CacheCandidate>('inspect_firestorm_cache', { path: selectedFile });
        if (version !== cacheRequestVersionRef.current || !cacheChooserActiveRef.current) return;
        selectedCacheCandidateRef.current = candidate;
        setSelectedCacheCandidate(candidate);
        setCacheCandidates((current) => [candidate, ...current.filter((entry) => entry.path !== candidate.path)]);
        setCacheDiscoveryBusy(false);
        const cachedNames = await invoke<Record<string, string>>('resolve_firestorm_avatar_names', { avatarIds: [candidate.avatarId] })
          .catch((): Record<string, string> => ({}));
        if (version !== cacheRequestVersionRef.current || !cacheChooserActiveRef.current) return;
        setFirestormAvatarNames((current) => ({ ...current, ...cachedNames }));
      }
    } catch (error) {
      if (version === cacheRequestVersionRef.current && cacheChooserActiveRef.current) setCacheDialogError(String(error));
    } finally {
      if (version === cacheRequestVersionRef.current) setCacheDiscoveryBusy(false);
    }
  }

  function saveSearch() {
    const label = savedSearchName.trim().slice(0, 80);
    if (!label) return;
    const record: SavedSearch = { label, query: query.slice(0, 300), kind, page: page === 'new' ? 'new' : 'find' };
    setPrefs((current) => ({ ...current, savedSearches: [record, ...current.savedSearches.filter((saved) => saved.label !== label)].slice(0, 20) }));
    setSavedSearchName('');
    setSaveSearchOpen(false);
    pushToast('Search saved on this device.');
  }

  async function saveTags() {
    if (!selected || tagSaveBusyRef.current) return;
    if (profileBusyRef.current || importBusyRef.current) { pushToast('Wait for the profile or import operation before saving tags.'); return; }
    const requestedAvatarId = avatar.id;
    const itemId = selected.id;
    const syncKey = `${requestedAvatarId}\u0000${itemId}`;
    const tags = Array.from(new Set(tagDraft.split(',').map((tag) => tag.trim().slice(0, 40)).filter(Boolean))).slice(0, 16);
    tagSaveBusyRef.current = true;
    setTagSaveBusy(true);
    setPrefs((current) => ({ ...current, itemTags: { ...current.itemTags, [itemId]: tags } }));
    setTagEditor(false);
    try {
      if (desktop && requestedAvatarId !== 'sample-avatar') {
        await invoke('save_item_tags', { avatarId: requestedAvatarId, itemId, tags });
        if (activeAvatarIdRef.current === requestedAvatarId) setTagSearchRevision((revision) => revision + 1);
      }
      setTagSyncErrors((current) => {
        const next = { ...current };
        delete next[syncKey];
        return next;
      });
      pushToast('Tags saved locally.');
    } catch (error) {
      setTagSyncErrors((current) => ({ ...current, [syncKey]: 'Desktop search could not update these tags. Open Add tags and save again to retry.' }));
      pushToast(`Tags kept in your local profile, but desktop search could not update. Save these tags again to retry: ${String(error)}`);
    } finally {
      tagSaveBusyRef.current = false;
      setTagSaveBusy(false);
    }
  }

  function toggleProtectedItem(itemId: string) {
    const wasProtected = prefs.protectedItemIds.includes(itemId);
    setPrefs((current) => ({
      ...current,
      protectedItemIds: wasProtected
        ? current.protectedItemIds.filter((id) => id !== itemId)
        : [...current.protectedItemIds, itemId],
    }));
    pushToast(wasProtected ? 'Keep-safe mark removed.' : 'Item marked keep safe on this device.');
  }

  function toggleProtectedFolder(folderId: string) {
    const wasProtected = prefs.protectedFolderIds.includes(folderId);
    setPrefs((current) => ({
      ...current,
      protectedFolderIds: wasProtected
        ? current.protectedFolderIds.filter((id) => id !== folderId)
        : [...current.protectedFolderIds, folderId],
    }));
    pushToast(wasProtected ? 'Folder keep-safe mark removed.' : 'Folder marked keep safe on this device.');
  }

  function clearProtectionMarks() {
    if (!prefs.protectedItemIds.length && !prefs.protectedFolderIds.length) return;
    if (!window.confirm('Remove all local keep-safe marks for this avatar? Inventory will not change.')) return;
    setPrefs((current) => ({ ...current, protectedItemIds: [], protectedFolderIds: [] }));
    pushToast('Local keep-safe marks cleared. Inventory was not changed.');
  }

  function openCreateCollectionDialog() {
    setCollectionDraft('');
    setCollectionOpen(false);
    setNewCollectionDialogOpen(true);
  }

  function createCollection(includeSelectedItem = Boolean(selected)) {
    const name = collectionDraft.trim().slice(0, 100);
    if (!name) return;
    const collectionItem = includeSelectedItem ? selected : null;
    const board = { id: crypto.randomUUID(), name, itemIds: collectionItem ? [collectionItem.id] : [] };
    setPrefs((current) => ({ ...current, collections: [board, ...current.collections] }));
    setCollectionDraft('');
    setCollectionOpen(false);
    setNewCollectionDialogOpen(false);
    setCollectionView(board.id);
    setPage('collections');
    pushToast(collectionItem ? 'Collection created with this item.' : 'Collection created on this device.');
  }

  function addToCollection(id: string) {
    if (!selected) return;
    if (prefs.collections.find((board) => board.id === id)?.itemIds.includes(selected.id)) {
      setCollectionOpen(false);
      pushToast('This item is already in that collection.');
      return;
    }
    setPrefs((current) => ({ ...current, collections: current.collections.map((board) => board.id === id
      ? { ...board, itemIds: Array.from(new Set([...board.itemIds, selected.id])) }
      : board) }));
    setCollectionOpen(false);
    pushToast('Added to collection.');
  }

  function removeFromCollection(collectionId: string, itemId: string) {
    setPrefs((current) => ({ ...current, collections: current.collections.map((board) => board.id === collectionId
      ? { ...board, itemIds: board.itemIds.filter((savedId) => savedId !== itemId) }
      : board) }));
    pushToast('Removed from collection.');
  }

  async function deleteCollection(collectionId: string) {
    const requestedAvatarId = avatar.id;
    await changeLocalPhoto(collectionImageKey(requestedAvatarId, collectionId), async () => {
      await removeLocalImage(collectionImageKey(requestedAvatarId, collectionId));
      const removeBoard = (current: LocalPreferences): LocalPreferences => ({ ...current, collections: current.collections.filter((board) => board.id !== collectionId) });
      if (activeAvatarIdRef.current === requestedAvatarId && prefsLoadedForRef.current === requestedAvatarId) {
        setPrefs(removeBoard);
        setCollectionView((current) => current === collectionId ? null : current);
      } else {
        // A native media transaction may complete after switching avatars.
        const updated = removeBoard(unsavedPreferencesRef.current.get(requestedAvatarId) ?? readPrefs(requestedAvatarId));
        try {
          localStorage.setItem(`shelfmark:${requestedAvatarId}`, JSON.stringify(updated));
          unsavedPreferencesRef.current.delete(requestedAvatarId);
        } catch {
          unsavedPreferencesRef.current.set(requestedAvatarId, updated);
          setPrefsSaveError('Collection changes need saving. Export a profile before closing Shelfmark.');
        }
      }
    }, 'Collection removed from this device.');
  }

  async function changeLocalPhoto(key: string, change: () => Promise<void>, message: string) {
    if (photoBusyRef.current || profileBusyRef.current) { pushToast('Wait for the current photo or profile operation to finish.'); return; }
    photoBusyRef.current = true;
    setPhotoBusy(true);
    if (activeItemPhotoKeyRef.current === key) photoRequestVersionRef.current += 1;
    try {
      await change();
      pushToast(message);
    } catch (error) {
      pushToast(error instanceof Error ? error.message : 'Could not update that local photo.');
    } finally {
      // Reload the current selection through its keyed effect. A completed save
      // must never assign a previous item's image to the newly selected item.
      setLocalMediaRevision((revision) => revision + 1);
      photoBusyRef.current = false;
      setPhotoBusy(false);
    }
  }

  async function addCover(event: React.ChangeEvent<HTMLInputElement>, id: string) {
    const file = event.target.files?.[0];
    event.target.value = '';
    if (!file) return;
    const key = collectionImageKey(avatar.id, id);
    await changeLocalPhoto(key, async () => saveLocalImage(key, await resizeImage(file)), 'Collection image saved on this device.');
  }

  async function addItemImage(event: React.ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0];
    event.target.value = '';
    if (!file || !selected) return;
    const key = itemImageKey(avatar.id, selected.id);
    await changeLocalPhoto(key, async () => saveLocalImage(key, await resizeImage(file)), `Photo saved for ${selected.name}.`);
  }

  async function keepThumbnailAsItemPhoto() {
    if (!selected || !selectedThumbnailUrl) return;
    const key = itemImageKey(avatar.id, selected.id);
    const thumbnailUrl = selectedThumbnailUrl;
    await changeLocalPhoto(key, async () => {
      const response = await fetch(thumbnailUrl);
      if (!response.ok) throw new Error('Could not read this local preview.');
      await saveLocalImage(key, await response.blob());
    }, `Firestorm preview saved for ${selected.name}.`);
  }

  async function removeItemImage() {
    if (!selected) return;
    const key = itemImageKey(avatar.id, selected.id);
    await changeLocalPhoto(key, () => removeLocalImage(key), `Photo removed for ${selected.name}.`);
  }

  async function copyFolderTrail(path: string[]) {
    try {
      if (!navigator.clipboard) throw new Error('The clipboard is unavailable.');
      await navigator.clipboard.writeText(path.join(' / '));
      pushToast('Folder trail copied.');
    } catch {
      pushToast('Could not copy the folder trail. Try again when Shelfmark is focused.');
    }
  }

  const nav = [
    { id: 'find' as Page, label: 'Find', icon: Search, count: avatar.itemCount },
    { id: 'folders' as Page, label: 'Folders', icon: FolderTree, count: avatar.id === 'sample-avatar' ? folderCatalog.length : avatar.categoryCount ?? 0 },
    { id: 'empty-folders' as Page, label: 'Empty folders', icon: FolderOpen, count: avatar.emptyFolderCount ?? 0 },
    { id: 'new' as Page, label: 'Recently added', icon: PackageOpen, count: recentCount },
    { id: 'collections' as Page, label: 'Collections', icon: Layers3, count: prefs.collections.length },
    { id: 'review' as Page, label: 'Candidate groups', icon: Archive, count: candidateCount },
  ];
  function navigateToPage(target: Page) {
    setPage(target);
    setCollectionView(null);
    setMobileDetailOpen(false);
    setKindMenu(false);
    setSortMenu(false);
    setAvatarMenu(false);
    if (target === 'folders' || target === 'empty-folders') {
      setSelectedFolderId(null);
      setFolderSearch('');
      setSelectedId('');
    }
    if (target === 'review' || target === 'collections') {
      setQuery('');
      setKind('Any type');
    }
    if (target === 'review') setCandidateFilter('All clues');
    if (target === 'new') setSortMode('Newest first');
    if (target === 'find' || target === 'review') setSortMode('Relevance');
  }
  function switchToViewerSnapshot(snapshot: AvatarOption) {
    setAvatar(snapshot);
    setSelectedFolderId(null);
    setFolderSearch('');
    setSelectedId('');
    setQuery('');
    setKind('Any type');
    navigateToPage('find');
    pushToast(`Switched to ${snapshot.label}, matching the active Firestorm avatar.`);
  }
  const selectedCandidate = selected ? candidateReason(selected) : null;
  const avatarMonogram = avatar.id === 'sample-avatar'
    ? 'S'
    : (avatarLabels[avatar.id]?.trim().slice(0, 1) || firestormAvatarNames[avatar.id]?.trim().slice(0, 1) || avatar.id.slice(0, 1)).toUpperCase();

  return (
    <div className="app-shell">
      <aside className="sidebar" inert={updateInstalling}>
        <div className="brand-row">
          <div className="brand-mark" aria-hidden="true"><span /><span /><span /><i /></div>
          <div className="brand-word">shelfmark<span>INVENTORY, REMEMBERED</span></div>
        </div>

        <div className="avatar-switcher-wrap">
          <button className="avatar-switcher" onClick={() => { if (avatarMenu) setAvatarLabelEditorOpen(false); setAvatarMenu(!avatarMenu); }} aria-expanded={avatarMenu}>
            <span className="avatar-monogram">{avatarMonogram}</span>
            <span className="avatar-label"><b>{avatar.label}</b><small>{avatar.id === 'sample-avatar' ? 'Sample catalogue' : 'Local Firestorm snapshot'}</small></span>
            <ChevronDown size={15} />
          </button>
          {avatarMenu && <div className="avatar-menu popover">
            <div className="popover-label">LOCAL AVATARS</div>
            <button className={`avatar-choice ${avatar.id === 'sample-avatar' ? 'selected' : ''}`} onClick={() => { setAvatar({ id: 'sample-avatar', label: 'Sample avatar', itemCount: sampleItems.length, emptyFolderCount: 1 }); setSelectedFolderId(null); setFolderSearch(''); setAvatarMenu(false); setAvatarLabelEditorOpen(false); }}>
              <span>Sample avatar</span><small>32 example items</small>
            </button>
            {avatars.map((option) => <button key={option.id} className={`avatar-choice ${avatar.id === option.id ? 'selected' : ''}`} onClick={() => { setAvatar(option); setSelectedFolderId(null); setFolderSearch(''); setAvatarMenu(false); setAvatarLabelEditorOpen(false); }}>
              <span>{option.label}</span><small>{option.itemCount.toLocaleString()} indexed items</small>
            </button>)}
            {avatar.id !== 'sample-avatar' && (avatarLabelEditorOpen ? <form className="avatar-label-editor" onSubmit={(event) => { event.preventDefault(); saveCurrentAvatarLabel(); }}>
              <label htmlFor="avatar-display-name">Display name for this device</label>
              <input id="avatar-display-name" autoFocus maxLength={48} value={avatarLabelDraft} onChange={(event) => setAvatarLabelDraft(event.target.value)} placeholder="e.g. Salty Spicy" />
              <div><button type="button" onClick={() => setAvatarLabelEditorOpen(false)}>Cancel</button><button type="submit">Save name</button></div>
            </form> : <button className="popover-action" onClick={() => { setAvatarLabelDraft(avatarLabels[avatar.id] ?? ''); setAvatarLabelEditorOpen(true); }}><Pencil size={13} /> Set avatar display name</button>)}
            <button className="popover-action" onClick={() => { setAvatarMenu(false); void importCache(); }}><Plus size={14} /> Import another avatar</button>
          </div>}
        </div>

        <div className="sidebar-scroll">
          {desktop && <>
            <div className="nav-caption">WORKSPACE</div>
            <nav className="primary-nav" aria-label="Workspace navigation">
              <button aria-label="Home" title="Home" className={`nav-item ${page === 'home' ? 'active' : ''}`} onClick={() => navigateToPage('home')}><House size={17} strokeWidth={1.8} /><span>Home</span></button>
            </nav>
          </>}
          <div className="nav-caption">YOUR DRAWERS</div>
          <nav className="primary-nav" aria-label="Inventory sections">
            {nav.map(({ id, label, icon: Icon, count }) => <button key={id} aria-label={label} title={label} className={`nav-item ${page === id ? 'active' : ''}`} onClick={() => navigateToPage(id)}>
              <Icon size={17} strokeWidth={1.8} />
              <span>{label}</span>
              {count !== undefined && <small>{count.toLocaleString()}</small>}
            </button>)}
          </nav>

          <div className="sidebar-rule" />
          <div className="nav-caption saved-caption">SAVED SEARCHES <button title="Save current search" onClick={() => setSaveSearchOpen(true)}><Plus size={15} /></button></div>
          <div className="saved-search-list">
            {prefs.savedSearches.map((saved, index) => <button className="saved-search" key={`${saved.label}-${index}`} onClick={() => { const targetPage = saved.page === 'new' ? 'new' : 'find'; setQuery(saved.query); setKind(saved.kind ?? 'Any type'); navigateToPage(targetPage); }}>
              <Bookmark size={14} /><span>{saved.label}</span>
            </button>)}
            {prefs.savedSearches.length === 0 && <small className="sidebar-empty">Searches you keep will live here.</small>}
          </div>
        </div>

        <div className="sidebar-bottom">
          <div className="privacy-note"><ShieldCheck size={16} /><div><b>Kept on this device</b><span>Inventory stays local.</span></div></div>
          <button className={`import-button ${busy ? 'working' : ''}`} aria-label={busy ? 'Reading Firestorm snapshot' : 'Import Firestorm cache'} title="Import Firestorm cache" onClick={() => void importCache()} disabled={busy}>
            {busy ? <span className="spinner" /> : <FolderOpen size={16} />}
            <span>{busy ? 'Reading snapshot…' : 'Import Firestorm cache'}</span>
          </button>
          <button className={`nav-item settings-nav ${page === 'settings' ? 'active' : ''}`} aria-label="Settings" title="Settings" onClick={() => navigateToPage('settings')}><Settings2 size={16} /><span>Settings</span></button>
        </div>
      </aside>

      <main className="main-panel" inert={updateInstalling}>
        <header className="topbar">
          <div className="topbar-context"><span className="crumb-root">SHELFMARK</span><span className="crumb-slash">/</span><span>{page === 'home' ? 'HOME' : page === 'find' ? 'CATALOGUE' : page === 'folders' ? 'FOLDERS' : page === 'empty-folders' ? 'EMPTY FOLDERS' : page === 'new' ? 'RECENTLY ADDED' : page === 'collections' ? 'COLLECTIONS' : page === 'review' ? 'REVIEW' : 'PREFERENCES'}</span></div>
          <div className="topbar-actions">
            {desktop && avatar.id !== 'sample-avatar' && <button className="quiet-button snapshot-refresh-button" onClick={() => void refreshAvatarSnapshot()} disabled={snapshotRefreshBusy || busy} title="Refresh this avatar from its Firestorm cache. Close all Firestorm windows first."><RefreshCw size={13} className={snapshotRefreshBusy ? 'spinning' : ''} /><span>{snapshotRefreshBusy ? 'Refreshing…' : 'Refresh snapshot'}</span></button>}
            <span className={`sample-badge ${avatar.id === 'sample-avatar' ? 'is-sample' : ''}`}><i />{avatar.id === 'sample-avatar' ? 'Sample data' : 'Local snapshot'}</span>
            <button className="icon-button theme-toggle" onClick={() => setPrefs((current) => ({ ...current, theme: current.theme === 'light' ? 'dark' : 'light' }))} aria-label={`Switch to ${prefs.theme === 'light' ? 'dark' : 'light'} theme`}>
              {prefs.theme === 'light' ? <Moon size={17} /> : <Sun size={17} />}
            </button>
            <button ref={helpTriggerRef} className="icon-button help-button" title="How to use Shelfmark" aria-label="How to use Shelfmark" onClick={() => setHelpOpen(true)}><CircleHelp size={17} /></button>
          </div>
        </header>

        <div className="content-scroll">
          {avatarListError && <div className="local-save-banner" role="alert"><CircleAlert size={17} /><div><b>Could not open local snapshots</b><p>{avatarListError}</p></div><button className="quiet-button" onClick={() => setAvatarListRetry((retry) => retry + 1)}>Retry</button></div>}
          {prefsSaveError && <div className="local-save-banner" role="alert"><CircleAlert size={17} /><div><b>Local changes need saving</b><p>{prefsSaveError}</p></div><button className="quiet-button" onClick={() => setPrefs((current) => ({ ...current }))}>Retry save</button><button className="primary-action" disabled={profileBusy || photoBusy} onClick={() => void exportProfile()}>Export profile</button></div>}
          {snapshotRefreshNeededFor === avatar.id && <div className="review-banner snapshot-refresh-banner"><RefreshCw size={15} /><span>Firestorm confirmed a live move and Shelfmark updated the item’s folder trail. Other snapshot data may stay unchanged until Firestorm saves its cache; close all viewer windows, then refresh.</span><button className="quiet-button" onClick={() => void refreshAvatarSnapshot()} disabled={snapshotRefreshBusy || busy}>{snapshotRefreshBusy ? 'Refreshing…' : 'Refresh snapshot'}</button></div>}
          {page === 'home' ? <HomePage desktop={desktop} avatar={avatar} viewerSnapshot={viewerSnapshotFor(avatars, stockLeapStatus.avatarId)} stockLeapStatus={stockLeapStatus} stockLeapAvatarLabel={stockLeapAvatarLabel} stockLeapBusy={stockLeapBusy} viewerLaunchError={viewerLaunchError} onStartStockLeap={() => void startStockFirestormLeap()} onImport={() => void importCache()} onSelectViewerSnapshot={switchToViewerSnapshot} onNavigate={navigateToPage} /> : page === 'settings' ? <SettingsPage desktop={desktop} avatar={avatar} summary={importSummary} stockLeapStatus={stockLeapStatus} stockLeapAvatarLabel={stockLeapAvatarLabel} stockLeapBusy={stockLeapBusy} stockLeapApiBusy={stockLeapApiBusy} stockLeapApiDescription={stockLeapApiDescription} stockLeapViewName={stockLeapViewName} stockLeapViewBusy={stockLeapViewBusy} stockLeapViewInspection={stockLeapViewInspection} onStockLeapViewNameChange={setStockLeapViewName} onInspectStockLeapView={() => void inspectStockFirestormInventoryView()} onDescribeStockLeapApi={(api) => void describeStockFirestormLeapApi(api)} onStartStockLeap={() => void startStockFirestormLeap()} onInspectStockLeap={() => void inspectStockFirestormLeap()} onScanStockLeapInventoryUi={() => void scanStockFirestormInventoryUi()} onImport={() => void importCache()} onHelp={() => setHelpOpen(true)} updateAvailable={pendingUpdate !== null} updateVersion={pendingUpdate?.version ?? null} updateStatus={updateStatus} updateBusy={updateBusy} onCheckUpdates={() => void checkForUpdates()} onInstallUpdate={() => void installAvailableUpdate()} profileBusy={profileBusy} onExportProfile={() => void exportProfile()} onImportProfile={() => profileBackupInputRef.current?.click()} protectedItemCount={prefs.protectedItemIds.length} protectedFolderCount={prefs.protectedFolderIds.length} onClearProtection={clearProtectionMarks} /> : page === 'collections' && !collectionView ? (
            <CollectionsPage mediaBusy={photoBusy || profileBusy} collections={prefs.collections} items={effectiveCatalog} coverUrls={collectionCoverUrls} canCreate={prefs.collections.length < 1000} onOpen={(id) => setCollectionView(id)} onCreate={openCreateCollectionDialog} onAddCover={addCover} onDelete={deleteCollection} />
          ) : (
            <>
              <section className="page-heading">
                <div>
                  <div className="eyebrow">{page === 'find' ? 'THE WHOLE INVENTORY, IN VIEW' : page === 'folders' ? 'THE FIRESTORM FOLDER TREE' : page === 'empty-folders' ? 'A SNAPSHOT SANITY CHECK' : page === 'new' ? 'A RECENT INVENTORY TRAIL' : page === 'review' ? 'SUGGESTIONS, NEVER DECISIONS' : collectionView ? 'A PERSONAL SHORTLIST' : 'A PERSONAL SHORTLIST'}</div>
                  <h1>{page === 'find' ? 'Find what you remember.' : page === 'folders' ? 'Browse by folder.' : page === 'empty-folders' ? 'Folders that look empty.' : page === 'new' ? 'What was added lately?' : page === 'review' ? 'Worth a second look.' : prefs.collections.find((board) => board.id === collectionView)?.name ?? 'Collection'}</h1>
                  <p>{page === 'find' ? 'Names fade. Folder trails help.' : page === 'folders' ? 'Open the saved hierarchy and browse its contents, one folder at a time.' : page === 'empty-folders' ? 'No direct items or subfolders were found in this imported snapshot.' : page === 'new' ? 'Recent inventory timestamps can include gifts and redeliveries.' : page === 'review' ? 'Text clues gather candidates. You decide what they mean.' : 'A quiet place to keep things together.'}</p>
                </div>
                <div className="inventory-caption"><span>{(page === 'folders' ? folderDisplayedTotal : page === 'empty-folders' ? emptyFoldersTotal : visibleItems.length).toLocaleString()}</span><small>{page === 'empty-folders' ? (emptyFoldersTotal === 1 ? 'folder appears empty' : 'folders appear empty') : (page === 'folders' ? folderDisplayedTotal : visibleItems.length) === 1 ? 'item in view' : 'items in view'}</small></div>
              </section>

              {page === 'collections' && collectionView && <div className="collection-backline"><button onClick={() => setCollectionView(null)}>‹ All collections</button><span>Local collection · {visibleItems.length} saved</span></div>}

              {page === 'empty-folders' && <div className="review-banner empty-folder-banner"><FolderOpen size={16} /><span>A partial cache can make folders look empty. Review each path in Firestorm before making changes; this view never deletes or moves anything.</span></div>}

              {page !== 'collections' && page !== 'review' && page !== 'folders' && page !== 'empty-folders' && <section className="search-row" aria-label="Search inventory">
                  <div className="search-input-wrap"><Search size={19} /><input ref={searchRef} value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search names, folders, notes or your tags" aria-label="Search inventory names, folders, notes or tags" /><kbd>{searchShortcut}</kbd>{query && <button className="clear-search" aria-label="Clear search" onClick={() => setQuery('')}><X size={15} /></button>}</div>
                <div className="filter-wrap">
                  <button className={`filter-button ${kind !== 'Any type' ? 'filter-active' : ''}`} onClick={() => setKindMenu(!kindMenu)} aria-expanded={kindMenu}><span>{kind}</span><ChevronDown size={14} /></button>
                  {kindMenu && <div className="kind-menu popover">{KIND_OPTIONS.map((option) => <button key={option} className={kind === option ? 'chosen' : ''} onClick={() => { setKind(option); setKindMenu(false); }}>{option}</button>)}</div>}
                </div>
                <button className="save-search-button" onClick={() => setSaveSearchOpen(true)}><Bookmark size={15} /><span>Save search</span></button>
              </section>}

              {page === 'review' && <>
                <div className="review-banner"><Sparkles size={16} /><span>These are clues from names and item types. A body-fit clue cannot tell what you still wear; duplicate names are never proof of identical assets.</span></div>
                <div className="candidate-filters" role="group" aria-label="Filter candidate clues">
                  {CANDIDATE_FILTERS.map((filter) => <button key={filter} className={candidateFilter === filter ? 'candidate-filter active' : 'candidate-filter'} onClick={() => setCandidateFilter(filter)} aria-pressed={candidateFilter === filter}>
                    <span>{CANDIDATE_LABELS[filter]}</span><small>{clueCounts[filter].toLocaleString()}</small>
                  </button>)}
                </div>
              </>}

              {page === 'collections' && collectionView && <CollectionTools mediaBusy={photoBusy || profileBusy} selected={selected} collectionId={collectionView} collections={prefs.collections} onRemove={removeFromCollection} onBrowse={() => { setPage('find'); setCollectionView(null); setQuery(''); setKind('Any type'); }} onUpload={(event) => void addCover(event, collectionView)} />}

              {avatar.id === 'sample-avatar' && <div className="sample-ribbon"><span><i /> Sample inventory</span><p>Fictional item names · {sampleItems.length.toLocaleString()} items · Safe to explore</p><button onClick={() => void importCache()}>Import your cache <span>→</span></button></div>}

              <section className={`workspace-grid ${page === 'empty-folders' ? 'empty-folders-grid' : ''}`}>
                {page === 'folders' ? <FolderBrowser
                  folderPaths={folderPathLabels}
                  folderProtection={folderSafety}
                  breadcrumbs={currentFolderTrail}
                  currentFolderId={selectedFolderId}
                  canSelectInFirestorm={desktop && avatar.id !== 'sample-avatar'}
                  viewerActionBusy={viewerActionBusy}
                  onSelectInFirestorm={(id, entryType) => void selectInFirestorm(id, entryType)}
                  childFolders={childFolders}
                  matches={folderMatches}
                  search={folderSearch}
                  onSearch={setFolderSearch}
                  onOpenFolder={(id) => { setSelectedFolderId(id); setSelectedId(''); setMobileDetailOpen(false); }}
                  loadingFolders={folderTreeLoading}
                  foldersError={folderTreeError}
                  onRetry={() => setFolderReadRevision((revision) => revision + 1)}
                  loadingItems={folderItemsLoading}
                  itemsError={folderItemsError}
                  items={folderViewItems}
                  selectedItemId={selected?.id ?? null}
                  totalItems={folderDisplayedTotal}
                  hasMore={folderItemsHasMore}
                  loadingMore={folderItemsLoadingMore}
                  onLoadMore={() => void loadMoreFolderItems()}
                  onSelectItem={selectInventoryItem}
                  onContextMenuItem={desktop ? openItemContextMenu : undefined}
                  onToggleProtectedFolder={toggleProtectedFolder}
                  protectedItemIds={protectedItemIds}
                /> : page === 'empty-folders' ? <EmptyFoldersReview
                  folders={emptyFolders.map((folder) => {
                    const safety = folderSafety.getSafety(folder.id);
                    return {
                      ...folder,
                      path: folderPathLabels.get(folder.id) ?? folder.name,
                      protected: safety.isProtected,
                      protectionReason: folderSafetyReasons(safety).join(' · '),
                    };
                  })}
                  loadedCount={emptyFolders.length}
                  totalCount={emptyFoldersTotal}
                  loading={emptyFoldersLoading || (desktop && folderTreeLoading)}
                  loadingMore={emptyFoldersLoadingMore}
                  hasMore={emptyFoldersHasMore}
                  error={emptyFoldersError || folderTreeError}
                  onRetry={() => { if (folderTreeError) setFolderReadRevision((revision) => revision + 1); void retryEmptyFolders(); }}
                  onLoadMore={() => void loadMoreEmptyFolders()}
                  onOpenFolder={(id) => { setPage('folders'); setSelectedFolderId(id); setFolderSearch(''); setSelectedId(''); setMobileDetailOpen(false); }}
                /> : <div className="results-panel">
                  <div className="results-head"><div><span className="section-kicker">{page === 'review' ? 'CANDIDATE MATCHES' : page === 'new' ? 'LATEST DATED ITEMS' : page === 'collections' ? 'SAVED ITEMS' : 'MATCHING ITEMS'}</span><small>{avatar.id === 'sample-avatar' ? `Sample · ${visibleItems.length} shown` : `Snapshot · ${visibleItems.length} shown${remoteMatchesView && remoteData?.hasMore ? ' · more available' : ''}`}</small></div>{!(page === 'collections' && collectionView) && <div className="sort-wrap"><button className="sort-button" aria-expanded={sortMenu} onClick={() => setSortMenu(!sortMenu)}><ArrowDownWideNarrow size={14} /> {sortMode}<ChevronDown size={12} /></button>{sortMenu && <div className="sort-menu popover" role="menu" aria-label="Sort results">{SORT_MODES.map((mode) => <button key={mode} role="menuitemradio" aria-checked={sortMode === mode} className={sortMode === mode ? 'chosen' : ''} onClick={() => { setSortMode(mode); setSortMenu(false); }}>{mode}</button>)}</div>}</div>}</div>
                  {remoteViewLoading ? <div className="empty-results loading-results"><span className="spinner local-spinner" /><b>{remoteData?.avatarId === avatar.id ? 'Searching this local snapshot…' : 'Opening this local snapshot…'}</b><p>Results stay on this device and separate from the other avatars in Shelfmark.</p></div> : visibleItems.length ? <div className="item-table" role="listbox" aria-label="Inventory results">
                    <div className="table-labels"><span>ITEM</span><span>FOLDER TRAIL</span><span>IN CACHE</span></div>
                  {visibleItems.map((item) => {
                    const safety = itemSafetyById.get(item.id)!;
                    return <ItemRow key={item.id} item={item} selected={selected?.id === item.id} query={query} page={page} protectedItem={safety.isProtected} protectionLabel={safety.reasons.map((reason) => reason.label).join(' · ')} onSelect={selectInventoryItem} onContextMenu={desktop ? openItemContextMenu : undefined} />;
                  })}
                  </div> : <div className="empty-results"><div className="empty-mark"><Search size={20} /></div><b>{remoteError ? 'Could not read this local index.' : page === 'review' ? `No ${candidateFilter === 'All clues' ? 'candidate matches' : CANDIDATE_LABELS[candidateFilter].toLowerCase()} found.` : query ? 'Nothing found by that phrase.' : 'Nothing in this view yet.'}</b><p>{remoteError ? 'The index remains on this device. Try switching avatars or importing that snapshot again.' : page === 'review' ? 'A clean result means the cached names and item types did not match this clue.' : query ? 'Try a folder name or remove a filter.' : avatar.id === 'sample-avatar' ? 'Choose a sample item or import your local Firestorm cache.' : 'Try a broader search.'}</p><button onClick={() => { setQuery(''); setKind('Any type'); if (page === 'review') setCandidateFilter('All clues'); else setPage('find'); }}>Clear filters</button></div>}
                  {remoteError && visibleItems.length > 0 && <div className="folder-feedback is-error" role="alert">Could not load more results. Your loaded results are still available.</div>}
                  {avatar.id !== 'sample-avatar' && remoteMatchesView && remoteData?.hasMore && <div className="load-more-row"><span>{visibleItems.length.toLocaleString()} loaded · more results available</span><button className="quiet-button" onClick={() => void loadMoreInventory()} disabled={remotePageLoading}>{remotePageLoading ? 'Loading next 100…' : remoteError ? 'Retry next page' : 'Load next 100 items'}</button></div>}
                </div>}

                {mobileDetailOpen && <div className="mobile-detail-scrim" aria-hidden="true" onClick={closeMobileDetail} />}
                {page !== 'empty-folders' && <aside className={`detail-panel ${mobileDetailOpen ? 'mobile-detail-open' : ''}`} role={mobileDetailOpen ? 'dialog' : undefined} aria-label="Selected inventory item" aria-labelledby="detail-item-title" aria-modal={mobileDetailOpen || undefined} onKeyDown={(event) => {
                  if (!mobileDetailOpen || event.key !== 'Tab') return;
                  const focusable = Array.from(event.currentTarget.querySelectorAll<HTMLElement>('button:not([disabled]), input:not([disabled]), textarea:not([disabled]), [href], [tabindex]:not([tabindex="-1"])')).filter((element) => element.getClientRects().length > 0);
                  if (!focusable.length) { event.preventDefault(); return; }
                  const lastFocusable = focusable[focusable.length - 1];
                  if (event.shiftKey && document.activeElement === focusable[0]) { event.preventDefault(); lastFocusable.focus(); }
                  else if (!event.shiftKey && document.activeElement === lastFocusable) { event.preventDefault(); focusable[0].focus(); }
                }}>
                  {selected ? <>
                    <div className="detail-topline"><span>ITEM DETAILS</span><div><button ref={mobileDetailCloseRef} className="mobile-detail-close icon-button" aria-label="Close item details" onClick={closeMobileDetail}><X size={15} /></button><button className="copy-path" onClick={() => void copyFolderTrail(selected.path)} title="Copy folder trail"><Clipboard size={14} /></button></div></div>
                    <div className="detail-title-block"><div className="item-glyph"><ItemIcon kind={selected.kind} /></div><div><h2 id="detail-item-title">{selected.name}</h2><span className="kind-label">{selected.kind}</span>{selected.isLink && <span className="link-label">Inventory link</span>}{selected.isFavorite && <span className="favorite-label"><Star size={11} fill="currentColor" /> Firestorm favorite</span>}</div></div>

                    <div className="detail-actions">
                      <button className="primary-action" onClick={() => setCollectionOpen(!collectionOpen)}><Bookmark size={15} /> Save to collection</button>
                      {desktop && avatar.id !== 'sample-avatar' && <>
                        <button className="viewer-action" onClick={() => void openMoveDialog()} disabled={ordinaryFolderMovesPaused || !moveBridgeSupported || !stockLeapMoveReady || !selected.parentFolderId || moveDialogLoading || moveBusy} title={ordinaryFolderMovesPaused ? 'Temporarily paused: stock LEAP cannot confirm clipboard contents. Move this item in Firestorm.' : !moveBridgeSupported ? 'Stock LEAP moves currently support the Second Life main grid only.' : !stockLeapMoveReady ? 'Launch Firestorm from Home and select the matching main-grid snapshot first.' : 'Choose a destination folder.'}>
                          <span className="viewer-action-label"><FolderOpen size={13} />{moveDialogLoading ? 'Loading folders…' : 'Move to folder…'}</span>
                          <span>{ordinaryFolderMovesPaused ? 'Paused' : stockLeapMoveReady ? 'Ready' : ''}</span>
                        </button>
                        <button className="viewer-action trash-action" onClick={() => void requestItemMove(null)} disabled={!moveBridgeSupported || !stockLeapMoveReady || !selected.parentFolderId || moveBusy} title={!moveBridgeSupported ? 'Stock LEAP actions currently support the Second Life main grid only.' : !stockLeapMoveReady ? 'Launch Firestorm from Home and select the matching main-grid snapshot first.' : 'Move to Trash immediately in Firestorm.'}>
                          <span className="viewer-action-label"><X size={13} />{moveBusy ? 'Sending request…' : 'Move to Trash'}</span>
                          <span>{stockLeapMoveReady ? 'Ready' : 'Connect first'}</span>
                        </button>
                        <small className="viewer-action-note">{moveBridgeSupported ? stockLeapMoveReady ? 'Firestorm confirms the Trash move before Shelfmark updates this view. Moves to other folders are paused.' : 'Connect Firestorm from Home and select the matching main-grid snapshot.' : 'Live Trash requires a Second Life main-grid snapshot.'}</small>
                        {moveFeedback?.itemId === selected.id && <div className={`move-feedback is-${moveFeedback.state}`} role={moveFeedback.state === 'error' ? 'alert' : 'status'}>
                          {moveFeedback.state === 'pending' ? <RefreshCw size={14} /> : moveFeedback.state === 'success' ? <CircleCheck size={14} /> : <CircleAlert size={14} />}
                          <span>{moveFeedback.message}</span>
                        </div>}
                      </>}
                      <button className="viewer-action" onClick={() => void selectInFirestorm(selected.id)} disabled={!desktop || avatar.id === 'sample-avatar' || !isInventoryUuid(selected.id) || viewerActionBusy} title="Select this UUID in the system-registered Second Life viewer; no inventory change is made."><span className="viewer-action-label"><ExternalLink size={13} />{viewerActionBusy ? 'Opening viewer…' : 'Select in Firestorm'}</span><span>{desktop && avatar.id !== 'sample-avatar' ? 'No inventory change' : 'Desktop snapshot required'}</span></button>
                      <small className="viewer-action-note">Uses your registered viewer. It must be logged in to this avatar and grid.</small>
                      {collectionOpen && <div className="collection-popover popover"><div className="popover-label">ADD TO COLLECTION</div>{prefs.collections.map((board) => <button key={board.id} onClick={() => addToCollection(board.id)}>{board.name}<small>{board.itemIds.length} saved</small></button>)}{prefs.collections.length < 1000 ? <div className="create-inline"><input maxLength={100} value={collectionDraft} onChange={(event) => setCollectionDraft(event.target.value)} placeholder="New collection"/><button onClick={() => createCollection(true)} disabled={!collectionDraft.trim()}><Plus size={14} /> Create</button></div> : <small>Profile collection limit reached.</small>}</div>}
                    </div>

                    <div className="detail-section keep-safe-section"><button className={`keep-safe-toggle ${selectedProtected ? 'is-protected' : ''}`} aria-pressed={selectedProtected} disabled={Boolean(selectedSafety?.isAutomaticallyProtected && !selectedProtectedDirectly) || (selectedProtectedByFolder && !selectedProtectedDirectly)} title={selectedSafetyCopy || 'A local exclusion for future review'} onClick={() => toggleProtectedItem(selected.id)}><span className="keep-safe-mark">{selectedProtected ? <ShieldCheck size={16} /> : <Shield size={16} />}</span><span><b>{selectedProtectedDirectly ? 'Marked keep safe' : selectedSafety?.isAutomaticallyProtected ? 'Kept safe automatically' : 'Keep this item safe'}</b><small>{selectedSafety?.isAutomaticallyProtected ? `${selectedSafetyCopy}. Snapshot clue only; no inventory change.` : selectedProtectedByFolder && !selectedProtectedDirectly ? 'Remove the folder mark in Folders · inventory unchanged' : 'Local exclusion for future review · no inventory change'}</small></span></button></div>

                    {selected.isLink && <div className="detail-section link-target-section"><span className="detail-label">LINK TARGET · THIS SNAPSHOT</span>{linkTargetLoading ? <p>Checking the local snapshot…</p> : linkTarget ? <div className="link-target-card"><b>{linkTarget.name}</b><small>{linkTarget.path.join(' / ') || 'Root folder'}</small><p>Firestorm exposes this reference in the selected cache. Shelfmark follows it only inside this avatar’s local snapshot.</p><button onClick={() => { setPage('folders'); setSelectedFolderId(linkTarget.isFolder ? linkTarget.id : linkTarget.parentFolderId === INVENTORY_ROOT_ID ? null : linkTarget.parentFolderId); setSelectedId(linkTarget.isFolder ? '' : linkTarget.id); setFolderSearch(''); setMobileDetailOpen(false); }}>Browse original folder</button></div> : <p>{linkTargetError ? 'Could not check the local snapshot.' : selected.linkedTargetId ? 'This target is not present in the selected snapshot.' : 'Firestorm did not expose a target UUID in this cache record.'}</p>}</div>}

                    <div className="detail-section personal-image-section">
                      <div className="personal-image-heading"><span className="detail-label">YOUR PHOTO</span><small>Saved on this device</small></div>
                      {selectedImageUrl
                        ? <img className="selected-item-image" src={selectedImageUrl} alt={`Your reference photo for ${selected.name}`} />
                        : selectedThumbnailUrl
                          ? <><img className="selected-item-image" src={selectedThumbnailUrl} alt={`Local Firestorm thumbnail for ${selected.name}`} /><small className="firestorm-preview-note">Firestorm cache preview · local only</small></>
                          : <div className="empty-item-image"><ImagePlus size={17} /><span>Add a photo or outfit reference.</span></div>}
                      <div className="personal-image-actions">
                        <label className="image-add-button"><ImagePlus size={14} /><span>{photoBusy ? 'Saving photo…' : selectedImageUrl ? 'Replace photo' : 'Add item photo'}</span><input ref={itemImageInputRef} disabled={photoBusy || profileBusy} type="file" accept="image/jpeg,image/png,image/webp" onChange={(event) => void addItemImage(event)} /></label>
                        {selectedThumbnailUrl && !selectedImageUrl && <button disabled={photoBusy || profileBusy} className="image-keep-preview" onClick={() => void keepThumbnailAsItemPhoto()}>Keep as item photo</button>}
                        {selectedImageUrl && <button disabled={photoBusy || profileBusy} className="delete-collection" aria-label="Remove item photo" title="Remove item photo" onClick={() => void removeItemImage()}><X size={14} /></button>}
                      </div>
                    </div>

                    <div className="detail-section path-section"><div className="detail-label-row"><span className="detail-label">FOLDER TRAIL</span>{selected.liveLocationConfirmedAt && <span className="live-location-badge" title={`Firestorm confirmed this location ${new Date(selected.liveLocationConfirmedAt).toLocaleString()}`}>LIVE · CACHE PENDING</span>}</div><div className="path-ribbon">{selected.path.length ? selected.path.map((segment, index) => <span key={`${segment}-${index}`} className={index === selected.path.length - 1 ? 'path-current' : ''}>{segment}{index < selected.path.length - 1 && <i>›</i>}</span>) : <span>Root folder</span>}</div></div>

                    <div className="detail-section tags-section"><div className="detail-label-row"><span className="detail-label">YOUR TAGS</span><button disabled={tagSaveBusy || busy || profileBusy} onClick={() => { setTagDraft((prefs.itemTags[selected.id] ?? selected.tags).join(', ')); setTagEditor(!tagEditor); }}>{tagSaveBusy ? 'Saving tags…' : tagEditor ? 'Cancel' : <><Plus size={13} /> Add tags</>}</button></div>
                      {tagEditor ? <form className="tag-form" onSubmit={(event) => { event.preventDefault(); void saveTags(); }}><input autoFocus maxLength={670} value={tagDraft} onChange={(event) => setTagDraft(event.target.value)} placeholder="wool, legacy, winter" aria-label="Comma-separated tags"/><button type="submit" disabled={tagSaveBusy || busy || profileBusy}>Save</button></form> : <div className="tag-list">{(prefs.itemTags[selected.id] ?? selected.tags).length ? (prefs.itemTags[selected.id] ?? selected.tags).map((tag) => <span key={tag} className="tag-chip">{tag}</span>) : <span className="no-tags">No tags yet</span>}</div>}
                    </div>

                    {tagSyncErrors[`${avatar.id}\u0000${selected.id}`] && <p className="tag-sync-error" role="alert">{tagSyncErrors[`${avatar.id}\u0000${selected.id}`]}</p>}
                    <div className="detail-section personal-note-section"><div className="personal-image-heading"><span className="detail-label">YOUR NOTE</span><small>Saved on this device</small></div><textarea maxLength={5000} aria-label={`Your private note for ${selected.name}`} value={prefs.itemNotes[selected.id] ?? ''} onChange={(event) => setPrefs((current) => ({ ...current, itemNotes: { ...current.itemNotes, [selected.id]: event.target.value } }))} placeholder="Fit notes, source, or pieces to pair with…" /></div>

                    <div className="detail-section"><span className="detail-label">QUICK NOTES</span><div className="meta-list">
                      <div><span>Creator</span><b>{selected.creatorId ? `UUID · ${selected.creatorId}` : 'Not in snapshot'}</b></div>
                      <div><span>Inventory timestamp</span><b>{formatDate(selected.createdAt)}</b></div>
                      <div><span>Snapshot</span><b>{avatar.cacheVersion ? `Firestorm format v${avatar.cacheVersion}` : 'Synthetic example'}</b></div>
                    </div></div>

                    <div className="detail-section permission-section"><div className="detail-label">PERMISSIONS · CACHED FLAGS</div><div className="permission-list">
                      <div><span>Current owner</span><b>{permissionSummary(selected.ownerMask)}</b></div>
                      <div><span>Next owner</span><b>{permissionSummary(selected.nextOwnerMask)}</b></div>
                    </div><p>Snapshot clues only. Firestorm and Second Life remain authoritative.</p></div>

                    {selectedCandidate && <div className="candidate-note"><span className={`confidence confidence-${selectedCandidate.confidence.toLowerCase()}`}>{selectedCandidate.confidence} clue</span><b>{selectedCandidate.label}</b><p>{selectedCandidate.reason}</p></div>}

                  </> : <div className="detail-empty"><Folder size={22} /><b>Choose an item</b><span>Details and folder trails will appear here.</span></div>}
                </aside>}
              </section>
              <div className="footer-note"><ShieldCheck size={14} /><span>Local index only</span><i />{desktop ? <span>Inventory and photos stay on this device.</span> : <span>The browser preview contains synthetic data.</span>}<i /><span>{desktop ? stockLeapMoveReady ? 'Live Trash is connected.' : 'Live Trash needs the matching viewer.' : 'Browser inventory stays read-only.'}</span></div>
            </>
          )}
        </div>
      </main>

      {saveSearchOpen && <div className="modal-scrim" onMouseDown={(event) => { if (event.target === event.currentTarget) setSaveSearchOpen(false); }}><form className="modal" onSubmit={(event) => { event.preventDefault(); saveSearch(); }}><div className="modal-top"><div><span className="eyebrow">KEEP A TRAIL</span><h2>Save this search</h2></div><button type="button" className="icon-button" onClick={() => setSaveSearchOpen(false)}><X size={17} /></button></div><p>Your search stays on this device and belongs to this avatar’s local catalogue.</p><label className="field-label" htmlFor="saved-search-name">Search name</label><input id="saved-search-name" autoFocus maxLength={80} value={savedSearchName} onChange={(event) => setSavedSearchName(event.target.value)} placeholder="e.g. Winter coats"/><div className="modal-actions"><button type="button" className="quiet-button" onClick={() => setSaveSearchOpen(false)}>Cancel</button><button type="submit" className="primary-action" disabled={!savedSearchName.trim()}>Save search</button></div></form></div>}

      {desktop && itemContextMenu && <div
        ref={itemContextMenuRef}
        className="item-context-menu"
        role="menu"
        aria-label={`Actions for ${itemContextMenu.item.name}`}
        onKeyDown={handleItemContextMenuKeyDown}
        onContextMenu={(event) => event.preventDefault()}
        style={{
          left: Math.max(8, Math.min(itemContextMenu.x, window.innerWidth - 260)),
          top: Math.max(8, Math.min(itemContextMenu.y, window.innerHeight - 220)),
        }}
      >
        <div className="item-context-heading"><b title={itemContextMenu.item.name}>{itemContextMenu.item.name}</b><small title={itemContextMenu.item.path.join(' / ')}>{itemContextMenu.item.path.join(' / ') || 'Inventory root'}</small></div>
        <button type="button" role="menuitem" disabled={ordinaryFolderMovesPaused || !moveBridgeSupported || !stockLeapMoveReady || !itemContextMenu.item.parentFolderId || moveBusy} title={ordinaryFolderMovesPaused ? 'Temporarily paused: stock LEAP cannot confirm clipboard contents. Move this item in Firestorm.' : !moveBridgeSupported ? 'Live moves currently support Second Life main-grid snapshots.' : !stockLeapMoveReady ? 'Connect Firestorm from Home and select the matching snapshot first.' : 'Choose where this item should go.'} onClick={() => { const item = itemContextMenu.item; setItemContextMenu(null); void openMoveDialog(item); }}><FolderOpen size={15} /><span>Move to folder…</span></button>
        <button type="button" role="menuitem" className="context-trash-action" disabled={!moveBridgeSupported || !stockLeapMoveReady || !itemContextMenu.item.parentFolderId || moveBusy} title={!moveBridgeSupported ? 'Live actions currently support Second Life main-grid snapshots.' : !stockLeapMoveReady ? 'Connect Firestorm from Home and select the matching snapshot first.' : 'Firestorm uses Delete for Trash; it does not paste the clipboard.'} onClick={() => { const item = itemContextMenu.item; setItemContextMenu(null); void requestItemMove(null, item); }}><X size={15} /><span>Move to Trash</span></button>
        <div className="item-context-divider" />
        <button type="button" role="menuitem" disabled={avatar.id === 'sample-avatar' || !isInventoryUuid(itemContextMenu.item.id) || viewerActionBusy} title="Select this item in Firestorm without changing inventory." onClick={() => { const item = itemContextMenu.item; setItemContextMenu(null); void selectInFirestorm(item.id); }}><ExternalLink size={15} /><span>Select in Firestorm</span></button>
        {ordinaryFolderMovesPaused && moveBridgeSupported && <small className="item-context-note">Moves to other folders are paused because stock LEAP cannot confirm the clipboard item. Trash uses Firestorm Delete.</small>}
        {!stockLeapMoveReady && <small className="item-context-note">Connect the matching Firestorm session from Home to enable live Trash actions.</small>}
      </div>}

      {moveDialogOpen && selected && <div className="modal-scrim" onMouseDown={(event) => { if (event.target === event.currentTarget && !moveBusy) setMoveDialogOpen(false); }}><section className="modal move-dialog" role="dialog" aria-modal="true" aria-labelledby="move-dialog-title">
        <div className="modal-top"><div><span className="eyebrow">LIVE FIRESTORM ACTION</span><h2 id="move-dialog-title">Move item</h2></div><button type="button" className="icon-button" aria-label="Close" onClick={() => setMoveDialogOpen(false)} disabled={moveBusy}><X size={17} /></button></div>
        <p><b>{selected.name}</b><br />From {selected.path.join(' / ') || 'Inventory root'}. Choose a destination folder. Firestorm will move it immediately.</p>
        <label className="field-label" htmlFor="move-folder-search">Find a destination</label>
        <input id="move-folder-search" autoFocus maxLength={160} value={moveFolderSearch} onChange={(event) => setMoveFolderSearch(event.target.value)} placeholder="Search folder names or paths" />
        <div className="move-folder-list" role="listbox" aria-label="Destination folders">
          {moveDestinationOptions.map(({ folder, path }) => <button type="button" role="option" aria-selected={moveDestinationId === folder.id} className={moveDestinationId === folder.id ? 'move-folder-option selected' : 'move-folder-option'} key={folder.id} onClick={() => setMoveDestinationId(folder.id)}><Folder size={14} /><span>{path}</span>{moveDestinationId === folder.id && <span className="move-selected-mark">Selected</span>}</button>)}
          {moveDestinationOptions.length === 0 && <div className="move-folder-empty">{moveFolderSearch ? 'No folders match that search.' : 'No destination folders are in this snapshot.'}</div>}
        </div>
        <small className="move-safety-note">Folder moves are temporarily paused. Stock Firestorm LEAP does not confirm which item is on the inventory clipboard after Cut, so a failed Cut could paste an older item. Move ordinary items inside Firestorm; Trash remains available through Firestorm Delete.</small>
        <div className="modal-actions"><button type="button" className="quiet-button" onClick={() => setMoveDialogOpen(false)} disabled={moveBusy}>Close</button><button type="button" className="primary-action" disabled title="Ordinary-folder moves are paused until stock Firestorm can confirm clipboard contents.">Unavailable</button></div>
      </section></div>}

      {newCollectionDialogOpen && <div className="modal-scrim" onMouseDown={(event) => { if (event.target === event.currentTarget) setNewCollectionDialogOpen(false); }}><form className="modal" onSubmit={(event) => { event.preventDefault(); createCollection(false); }}><div className="modal-top"><div><span className="eyebrow">KEEP YOUR FINDS TOGETHER</span><h2>New collection</h2></div><button type="button" className="icon-button" aria-label="Close" onClick={() => setNewCollectionDialogOpen(false)}><X size={17} /></button></div><p>Collections and outfit references stay on this device.</p><label className="field-label" htmlFor="collection-name">Collection name</label><input id="collection-name" autoFocus maxLength={100} value={collectionDraft} onChange={(event) => setCollectionDraft(event.target.value)} placeholder="e.g. Everyday layers"/><div className="modal-actions"><button type="button" className="quiet-button" onClick={() => setNewCollectionDialogOpen(false)}>Cancel</button><button className="primary-action" type="submit" disabled={!collectionDraft.trim() || prefs.collections.length >= 1000}>Create collection</button></div></form></div>}

      {cacheDialogOpen && <CacheDiscoveryDialog avatarLabels={avatarLabels} avatarNames={firestormAvatarNames} candidates={cacheCandidates} viewerRunning={firestormRunning} cacheDirectories={firestormCacheDirectories} selectedCandidate={selectedCacheCandidate} discoveryBusy={cacheDiscoveryBusy} importing={busy} progress={importProgress} cancellationPending={importCancellationPending} error={cacheDialogError} onClose={closeCacheChooser} onSelect={(candidate) => { selectedCacheCandidateRef.current = candidate; setSelectedCacheCandidate(candidate); }} onImportSelected={(candidate, label) => void importCachePath(candidate.path, label)} onCancelImport={() => void cancelInventoryImport()} onBrowse={() => void browseForCache()} onRefresh={() => void refreshCacheDiscovery()} />}
      {helpOpen && <GuideDialog desktop={desktop} onClose={() => { setHelpOpen(false); window.requestAnimationFrame(() => helpTriggerRef.current?.focus()); }} onImport={() => { setHelpOpen(false); void importCache(); }} />}
      <input ref={profileBackupInputRef} className="profile-backup-input" type="file" accept=".json,application/json" onChange={(event) => void readProfileFile(event)} aria-label="Choose a Shelfmark profile file" />
      {profileRestore && <ProfileRestoreDialog backup={profileRestore} busy={profileBusy} onCancel={() => setProfileRestore(null)} onRestore={() => void applyProfileRestore()} />}
      {updateInstalling && <UpdateProgressDialog status={updateStatus} />}
      {toast && <div className="toast-message" role="status">{toast}</div>}
    </div>
  );
}

function isTyping(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && (['INPUT', 'TEXTAREA'].includes(target.tagName) || target.isContentEditable);
}

function ItemIcon({ kind }: { kind: ItemKind }) {
  if (kind === 'Notecard') return <Clipboard size={16} />;
  if (kind === 'Landmark') return <span className="landmark-pin">⌖</span>;
  if (kind === 'Folder') return <Folder size={16} />;
  if (kind === 'Object') return <PackageOpen size={16} />;
  return <span className="garment-glyph">✳</span>;
}

const ItemRow = memo(function ItemRow({ item, selected, query, page, onSelect, onContextMenu, protectedItem = false, protectionLabel = '' }: { item: InventoryItem; selected: boolean; query: string; page: Page; onSelect: (item: InventoryItem) => void; onContextMenu?: (item: InventoryItem, event: ReactMouseEvent<HTMLButtonElement>) => void; protectedItem?: boolean; protectionLabel?: string }) {
  const reason = candidateReason(item);
  const matchingPath = query && reason ? item.path.find((part) => part.toLowerCase().includes(query.toLowerCase())) : undefined;
  return <button role="option" aria-selected={selected} className={`item-row ${selected ? 'row-selected' : ''}`} onClick={() => onSelect(item)} onContextMenu={(event) => onContextMenu?.(item, event)}>
    <span className={`row-kind kind-${item.kind.toLowerCase()}`}><ItemIcon kind={item.kind} /></span>
    <span className="row-main"><span className="row-title"><b>{item.name}</b>{protectedItem && <span className="row-safety-mark" role="img" aria-label={`Kept safe: ${protectionLabel || 'local review exclusion'}`} title={protectionLabel || 'Local review exclusion'}><ShieldCheck size={13} aria-hidden="true" /></span>}</span><small>{item.creatorId ? `Creator · ${item.creatorId}` : item.kind}</small></span>
    <span className="row-path">{item.path.slice(-2).map((part, index) => <span key={`${part}-${index}`} className={part === matchingPath ? 'match-crumb' : ''}>{part}</span>)}</span>
    <span className="row-date">{formatDate(item.createdAt)}</span>
    {page === 'review' && reason && <span className={`row-confidence confidence-${reason.confidence.toLowerCase()}`}>{reason.confidence}</span>}
  </button>;
});

function EmptyFoldersReview({ folders, loadedCount, totalCount, loading, loadingMore, hasMore, error, onRetry, onLoadMore, onOpenFolder }: {
  folders: (FolderSummary & { path: string; protected: boolean; protectionReason: string })[];
  loadedCount: number;
  totalCount: number;
  loading: boolean;
  loadingMore: boolean;
  hasMore: boolean;
  error: string;
  onRetry: () => void;
  onLoadMore: () => void;
  onOpenFolder: (folderId: string) => void;
}) {
  return <div className="results-panel empty-folder-panel">
    <div className="results-head"><div><span className="section-kicker">EMPTY FOLDER CLUES</span><small>{loadedCount.toLocaleString()} of {totalCount.toLocaleString()} leaf folders · current snapshot only</small></div></div>
    {loading ? <div className="empty-results loading-results"><span className="spinner local-spinner" /><b>Reading the selected snapshot…</b><p>Folder names and paths stay on this device.</p></div>
      : error && !loadedCount ? <div className="empty-results"><div className="empty-mark"><CloudOff size={20} /></div><b>Could not read this local index.</b><p>The snapshot remains on this device. Try loading this view again.</p><button onClick={onRetry}>Try again</button></div>
        : folders.length ? <div className="empty-folder-list" role="list" aria-label="Folders that appear empty in this snapshot">
          {folders.map((folder) => <article className="empty-folder-row" role="listitem" key={folder.id}>
            <span className="empty-folder-icon"><Folder size={17} /></span>
            <div className="empty-folder-copy"><b>{folder.name}</b><small>{folder.path}</small></div>
            <span className={`empty-folder-status ${folder.protected ? 'is-protected' : ''}`} aria-label={folder.protected ? `Kept safe: ${folder.protectionReason}` : 'Appears empty in this snapshot'} title={folder.protected ? folder.protectionReason : 'No automatic or local keep-safe clue was found in this snapshot'}>{folder.protected ? <><ShieldCheck size={13} /> Kept safe</> : 'Appears empty'}</span>
            <button className="quiet-button" onClick={() => onOpenFolder(folder.id)}>Open in Folders <span aria-hidden="true">→</span></button>
          </article>)}
        </div> : <div className="empty-results"><div className="empty-mark"><FolderOpen size={20} /></div><b>No empty leaf folders found.</b><p>This snapshot has no folders without both direct items and subfolders. A partial cache can still hide contents.</p></div>}
    {error && loadedCount > 0 && <div className="folder-feedback is-error" role="alert">Could not load more folders. Your loaded folders are still available.</div>}
    {hasMore && !loading && <div className="load-more-row"><span>{loadedCount.toLocaleString()} loaded · {totalCount.toLocaleString()} found</span><button className="quiet-button" onClick={onLoadMore} disabled={loadingMore}>{loadingMore ? 'Loading more…' : error ? 'Retry next page' : 'Load next 100 folders'}</button></div>}
  </div>;
}

function FolderBrowser({
  folderPaths, folderProtection, breadcrumbs, currentFolderId, canSelectInFirestorm, viewerActionBusy, onSelectInFirestorm, childFolders, matches, search, onSearch, onOpenFolder,
  loadingFolders, foldersError, loadingItems, itemsError, items, selectedItemId, totalItems,
  hasMore, loadingMore, onLoadMore, onRetry, onSelectItem, onContextMenuItem, onToggleProtectedFolder, protectedItemIds,
}: {
  folderPaths: ReadonlyMap<string, string>; folderProtection: ReturnType<typeof createFolderProtectionResolver>; breadcrumbs: FolderSummary[]; currentFolderId: string | null; canSelectInFirestorm: boolean; viewerActionBusy: boolean;
  onSelectInFirestorm: (folderId: string, entryType: 'folder') => void; childFolders: FolderSummary[];
  matches: FolderSummary[]; search: string; onSearch: (value: string) => void; onOpenFolder: (id: string | null) => void;
  loadingFolders: boolean; foldersError: string; loadingItems: boolean; itemsError: string; items: InventoryItem[];
  selectedItemId: string | null; totalItems: number; hasMore: boolean; loadingMore: boolean; onLoadMore: () => void; onRetry: () => void;
  onSelectItem: (item: InventoryItem) => void; onContextMenuItem?: (item: InventoryItem, event: ReactMouseEvent<HTMLButtonElement>) => void;
  onToggleProtectedFolder: (folderId: string) => void; protectedItemIds: ReadonlySet<string>;
}) {
  const showingMatches = Boolean(search.trim());
  const directoryKey = `${currentFolderId ?? INVENTORY_ROOT_ID}\u0000${search.trim().toLocaleLowerCase()}`;
  const [directoryPage, setDirectoryPage] = useState({ key: '', limit: INVENTORY_PAGE_SIZE });
  const folderLimit = directoryPage.key === directoryKey ? directoryPage.limit : INVENTORY_PAGE_SIZE;
  const directoryCount = showingMatches ? matches.length : childFolders.length;
  const itemSafetyById = useMemo(() => new Map(items.map((item) => [item.id, classifyItemSafety(item, protectedItemIds, folderProtection)])), [items, protectedItemIds, folderProtection]);
  const currentFolderIsProtected = currentFolderId
    ? folderProtection.isProtected(currentFolderId)
    : false;
  const currentFolderIsProtectedDirectly = Boolean(currentFolderId && folderProtection.isMarked(currentFolderId));
  const currentFolderSafety = currentFolderId ? folderProtection.getSafety(currentFolderId) : null;
  const currentFolderSafetyReason = currentFolderSafety ? folderSafetyReasons(currentFolderSafety).join(' · ') : '';
  return <section className="folder-explorer results-panel" aria-label="Browse inventory folders">
    <div className="folder-explorer-toolbar">
      <nav className="folder-breadcrumbs" aria-label="Current folder">
        <button aria-current={currentFolderId ? undefined : 'page'} onClick={() => onOpenFolder(null)}>Inventory root</button>
        {breadcrumbs.map((folder) => <span className="folder-crumb" key={folder.id}><i aria-hidden="true">›</i><button aria-current={folder.id === currentFolderId ? 'page' : undefined} onClick={() => onOpenFolder(folder.id)}>{folder.name}</button></span>)}
      </nav>
      <label className="folder-search"><Search size={16} /><input value={search} onChange={(event) => onSearch(event.target.value)} placeholder="Search folders by name or path" aria-label="Search inventory folders" />{search && <button aria-label="Clear folder search" onClick={() => onSearch('')}><X size={14} /></button>}</label>
    </div>

    {loadingFolders ? <div className="folder-feedback"><span className="spinner local-spinner" /><span>Opening this avatar’s folder tree…</span></div> : foldersError ? <div className="folder-feedback is-error" role="alert"><span>Could not read this avatar’s folders. {foldersError}</span><button className="quiet-button" onClick={onRetry}>Try again</button></div> : <>
      {showingMatches ? <section className="folder-directory">
        <div className="folder-directory-head"><div><span className="section-kicker">FOLDER MATCHES</span><small>Open a result to browse its contents.</small></div><span>{matches.length.toLocaleString()}</span></div>
        {matches.length ? <div className="folder-match-list">{matches.slice(0, folderLimit).map((folder) => { const safety = folderProtection.getSafety(folder.id); const protectionReason = folderSafetyReasons(safety).join(' · '); return <button className="folder-match-row" key={folder.id} onClick={() => { onOpenFolder(folder.id); onSearch(''); }}><Folder size={16} /><span><b>{folder.name}</b><small>{folderPaths.get(folder.id) ?? folder.name}</small></span>{safety.isProtected && <span className="folder-safe-label" aria-label={`Kept safe: ${protectionReason}`} title={protectionReason}><ShieldCheck size={14} /></span>}<span>Open</span></button>; })}</div> : <div className="folder-feedback">No folders match “{search.trim()}”.</div>}
      </section> : <section className="folder-directory">
        <div className="folder-directory-head"><div><span className="section-kicker">FOLDERS HERE</span><small>{currentFolderId ? 'Folders nested inside this location.' : 'Choose a top-level folder to begin.'}</small></div><span>{childFolders.length.toLocaleString()}</span></div>
        {childFolders.length ? <div className="folder-child-list">{childFolders.slice(0, folderLimit).map((folder) => { const safety = folderProtection.getSafety(folder.id); const protectionReason = folderSafetyReasons(safety).join(' · '); return <button className="folder-child-row" key={folder.id} onClick={() => onOpenFolder(folder.id)}><span className="folder-child-icon"><Folder size={16} /></span><span><b>{folder.name}</b>{folder.preferredType && folder.preferredType !== '-1' && <small>{safety.systemFolder ? folderSafetyLabel(safety.systemFolder) : folder.preferredType}</small>}</span>{safety.isProtected && <span className="folder-safe-label" aria-label={`Kept safe: ${protectionReason}`} title={protectionReason}><ShieldCheck size={14} /></span>}<span className="folder-open-label">Open <span aria-hidden="true">›</span></span></button>; })}</div> : <div className="folder-feedback">There are no subfolders here.</div>}
      </section>}

      {directoryCount > folderLimit && <div className="load-more-row"><span>{folderLimit.toLocaleString()} of {directoryCount.toLocaleString()} folders shown</span><button className="quiet-button" onClick={() => setDirectoryPage({ key: directoryKey, limit: folderLimit + INVENTORY_PAGE_SIZE })}>Show next 100 folders</button></div>}

      <section className="folder-items-section" aria-label="Items in the selected folder">
        <div className="folder-items-heading"><div><span className="section-kicker">ITEMS IN THIS FOLDER</span><small>{currentFolderId ? breadcrumbs[breadcrumbs.length - 1]?.name : 'Inventory root'}</small></div><div className="folder-items-heading-actions"><span>{totalItems.toLocaleString()}</span>{currentFolderId && canSelectInFirestorm && isInventoryUuid(currentFolderId) && <button className="icon-button folder-locate-button" aria-label="Select this folder in Firestorm" title="Select this folder in Firestorm. This does not move or change inventory." disabled={viewerActionBusy} onClick={() => onSelectInFirestorm(currentFolderId, 'folder')}><ExternalLink size={15} /></button>}{currentFolderId && <button className={`keep-safe-button ${currentFolderIsProtected ? 'is-protected' : ''}`} aria-pressed={currentFolderIsProtected} disabled={currentFolderIsProtected && !currentFolderIsProtectedDirectly} title={currentFolderSafetyReason || 'Local exclusion for future review; this does not move inventory'} onClick={() => onToggleProtectedFolder(currentFolderId)}>{currentFolderIsProtectedDirectly ? <ShieldCheck size={13} /> : currentFolderIsProtected ? <ShieldCheck size={13} /> : <Shield size={13} />}{currentFolderIsProtectedDirectly ? 'Kept safe' : currentFolderSafety?.systemFolder ? 'System folder' : currentFolderIsProtected ? 'Protected by parent' : 'Keep folder safe'}</button>}</div></div>
        {itemsError && items.length > 0 && <div className="folder-feedback is-error" role="alert">Could not load more items. Your loaded items are still available.</div>}
        {loadingItems ? <div className="folder-feedback"><span className="spinner local-spinner" /><span>Loading items in this folder…</span></div> : itemsError && !items.length ? <div className="folder-feedback is-error" role="alert"><span>Could not read the selected folder. {itemsError}</span><button className="quiet-button" onClick={onRetry}>Try again</button></div> : items.length ? <>
          <div className="table-labels folder-table-labels"><span>ITEM</span><span>FOLDER TRAIL</span><span>IN CACHE</span></div>
          <div className="item-table" role="listbox" aria-label="Items in the selected folder">{items.map((item) => { const safety = itemSafetyById.get(item.id)!; return <ItemRow key={item.id} item={item} selected={selectedItemId === item.id} query="" page="folders" protectedItem={safety.isProtected} protectionLabel={safety.reasons.map((reason) => reason.label).join(' · ')} onSelect={onSelectItem} onContextMenu={onContextMenuItem} />; })}</div>
          {hasMore && <div className="load-more-row"><span>{items.length.toLocaleString()} of {totalItems.toLocaleString()} items</span><button className="quiet-button" onClick={onLoadMore} disabled={loadingMore}>{loadingMore ? 'Loading…' : itemsError ? 'Retry next page' : 'Load next 100'}</button></div>}
        </> : <div className="folder-feedback">This folder has no direct items. Open a subfolder above or choose another folder.</div>}
      </section>
    </>}
  </section>;
}

function CollectionsPage({
  collections, items, coverUrls, canCreate, mediaBusy, onOpen, onCreate, onAddCover, onDelete,
}: {
  collections: LocalPreferences['collections']; items: InventoryItem[]; coverUrls: Record<string, string>; onOpen: (id: string) => void;
  canCreate: boolean; mediaBusy: boolean; onCreate: () => void; onAddCover: (event: React.ChangeEvent<HTMLInputElement>, id: string) => void; onDelete: (id: string) => void;
}) {
  return <>
    <section className="page-heading collections-heading"><div><div className="eyebrow">A DRAWER OF YOUR OWN</div><h1>Keep a few things close.</h1><p>Small, visual shortlists you curate for yourself.</p></div><button className="primary-action" onClick={onCreate} disabled={!canCreate}><Plus size={15} /> New collection</button></section>
    <div className="collections-list">{collections.map((board, index) => {
      const coverItem = items.find((item) => board.itemIds.includes(item.id));
      return <article className="collection-row" key={board.id}>
        <button className={`collection-cover cover-${index % 4}`} onClick={() => onOpen(board.id)} aria-label={`Open ${board.name}`}>
          {coverUrls[board.id] || board.cover ? <img src={coverUrls[board.id] ?? board.cover} alt="User supplied collection cover" /> : <><span className="cover-mark">{coverItem ? <ItemIcon kind={coverItem.kind} /> : <Layers3 size={18} />}</span><i /><i /><i /></>}
        </button>
        <button className="collection-copy" onClick={() => onOpen(board.id)}><b>{board.name}</b><span>{board.itemIds.length} saved {board.itemIds.length === 1 ? 'item' : 'items'}<i />Private to this device</span></button>
        <div className="collection-row-actions"><label className="image-add-button"><ImagePlus size={15} /><span>Add cover image</span><input disabled={mediaBusy} type="file" accept="image/jpeg,image/png,image/webp" onChange={(event) => onAddCover(event, board.id)} /></label><button disabled={mediaBusy} className="delete-collection" title={`Delete ${board.name}`} aria-label={`Delete ${board.name}`} onClick={() => { if (window.confirm(`Delete “${board.name}” and its local cover image? Inventory items will not change.`)) onDelete(board.id); }}><X size={14} /></button></div>
      </article>;
    })}{collections.length === 0 && <div className="empty-collections"><div className="empty-mark"><Layers3 size={19} /></div><b>Your collections start here.</b><p>Keep a small outfit shortlist, a set of home finds, or anything you want to rediscover. Collections never move inventory.</p><button className="primary-action" onClick={onCreate} disabled={!canCreate}><Plus size={14} /> Create a collection</button></div>}</div>
    <div className={`collections-note ${collections.length === 0 ? 'is-empty' : ''}`}><ImagePlus size={15} /><span>Photos are resized and saved on this device. They are never uploaded.</span></div>
  </>;
}

function CollectionTools({ selected, collectionId, collections, mediaBusy, onRemove, onBrowse, onUpload }: {
  selected: InventoryItem | null; collectionId: string; collections: LocalPreferences['collections']; mediaBusy: boolean;
  onRemove: (collectionId: string, itemId: string) => void; onBrowse: () => void; onUpload: (event: React.ChangeEvent<HTMLInputElement>) => void;
}) {
  const collection = collections.find((board) => board.id === collectionId);
  return <div className="collection-toolbar"><span><Bookmark size={14} /> {collection?.itemIds.length ?? 0} saved items</span><div><label className="image-add-button"><ImagePlus size={14} /><span>Cover image</span><input disabled={mediaBusy} type="file" accept="image/jpeg,image/png,image/webp" onChange={onUpload} /></label><button className="quiet-button" onClick={onBrowse}><Plus size={14} /> Find items to add</button>{selected && <button className="quiet-button remove-saved-item" onClick={() => onRemove(collectionId, selected.id)}><X size={14} /> Remove selected</button>}</div></div>;
}

function HomePage({ desktop, avatar, viewerSnapshot, stockLeapStatus, stockLeapAvatarLabel, stockLeapBusy, viewerLaunchError, onStartStockLeap, onImport, onSelectViewerSnapshot, onNavigate }: {
  desktop: boolean; avatar: AvatarOption; viewerSnapshot: AvatarOption | null; stockLeapStatus: StockLeapStatus; stockLeapAvatarLabel: string | null; stockLeapBusy: boolean; viewerLaunchError: string;
  onStartStockLeap: () => void; onImport: () => void; onSelectViewerSnapshot: (snapshot: AvatarOption) => void; onNavigate: (page: Page) => void;
}) {
  const connected = desktop && stockLeapStatus.connected;
  const hasSnapshot = avatar.id !== 'sample-avatar';
  const matchesSnapshot = connected && supportsStockLeapMove(avatar.id) && stockLeapStatus.avatarId?.toLowerCase() === avatar.id.split('@')[0].toLowerCase();
  const moveApisReady = ['LLAgent', 'LLCommandDispatcher', 'LLInventory', 'LLWindow'].every((api) => stockLeapStatus.apiNames.includes(api));
  const movesReady = Boolean(matchesSnapshot && moveApisReady && !stockLeapBusy);
  const connectionError = !connected ? viewerLaunchError || (stockLeapStatus.phase === 'error' ? stockLeapStatus.message : '') : '';
  const primaryLabel = connected && stockLeapBusy ? 'Checking session…'
    : !connected ? stockLeapStatus.phase === 'starting' ? 'Starting Firestorm…' : connectionError ? 'Retry launch' : 'Launch Firestorm'
      : matchesSnapshot ? 'Open inventory' : viewerSnapshot ? 'Switch avatar snapshot' : 'Import avatar snapshot';
  const primaryAction = !connected
    ? onStartStockLeap
    : matchesSnapshot ? () => onNavigate('find')
      : viewerSnapshot ? () => onSelectViewerSnapshot(viewerSnapshot)
        : onImport;
  const primaryDisabled = (!connected && (stockLeapBusy || stockLeapStatus.phase === 'starting'))
    || (connected && stockLeapBusy);
  const activeAvatar = connected ? stockLeapAvatarLabel ?? 'Avatar connected' : hasSnapshot ? avatar.label : 'No avatar snapshot yet';
  const avatarInitial = activeAvatar.trim().charAt(0).toUpperCase() || 'S';
  const snapshotTime = avatar.snapshotAt ? relativeTime(avatar.snapshotAt) : 'Not imported yet';
  const moveStatus = stockLeapBusy && connected ? 'Checking viewer'
    : movesReady ? 'Trash ready'
        : connected && !matchesSnapshot ? 'Snapshot needed' : connected ? 'Viewer check needed' : 'Starts with Firestorm';
  const launchDescription = connected && stockLeapBusy
    ? 'Shelfmark is reading the active avatar and move APIs. The session finishes connecting automatically.'
    : connected && matchesSnapshot
      ? 'Your active avatar matches this local snapshot. Open the catalogue and pick up where you left off.'
      : connected && viewerSnapshot
        ? `A snapshot for ${stockLeapAvatarLabel ?? 'this avatar'} is ready. Switch to it to continue.`
        : connected
          ? `No local snapshot is selected for ${stockLeapAvatarLabel ?? 'this avatar'} yet. Import one after confirming the avatar.`
          : 'Launch Firestorm here. Shelfmark detects its avatar and connects the session automatically.';

  return <section className="home-page desktop-home" aria-labelledby="home-title">
    <div className="home-intro">
      <div>
        <span className="eyebrow">YOUR SHELFMARK LAUNCHPAD</span>
        <h1 id="home-title">Your avatar’s closet, ready.</h1>
        <p>Start Firestorm, land on the right avatar, and jump straight back into its inventory.</p>
      </div>
    </div>

    <section className={`home-launcher ${connected ? 'is-connected' : ''}`} aria-label="Firestorm launcher">
      <div className="home-launcher-topline">
        <span className={`home-connection-label ${connected ? 'online' : ''}`}><i />{connected && stockLeapBusy ? 'CHECKING SESSION' : connected ? 'FIRESTORM CONNECTED' : stockLeapStatus.phase === 'starting' ? 'STARTING FIRESTORM' : 'FIRESTORM SESSION'}</span>
        <span className="home-local-chip"><FolderOpen size={12} /> LOCAL INVENTORY</span>
      </div>
      <div className="home-launcher-stage">
        <div className={`launcher-avatar-mark ${connected && !matchesSnapshot ? 'needs-snapshot' : ''}`} aria-hidden="true"><span>{avatarInitial}</span><small>AVATAR</small></div>
        <div className="launcher-avatar-copy">
          <span className="launcher-kicker">{connected ? 'ACTIVE AVATAR' : 'SELECTED SNAPSHOT'}</span>
          <h2>{activeAvatar}</h2>
          <p>{launchDescription}</p>
        </div>
        <div className="launcher-action">
          <button className="home-primary launcher-primary" onClick={primaryAction} disabled={primaryDisabled}>
            <span className="launcher-play-icon">{connected ? <FolderOpen size={18} /> : <Play size={17} fill="currentColor" />}</span>
            <span className="launcher-primary-copy"><b>{primaryLabel}</b><small>{connected && matchesSnapshot ? 'RETURN TO INVENTORY' : 'START YOUR SESSION'}</small></span>
            <ArrowRight size={17} />
          </button>
          <small>{connected && matchesSnapshot ? 'Snapshot ready to open' : connected ? 'Choose or import the matching snapshot' : 'Connects to Shelfmark automatically'}</small>
        </div>
      </div>
      {connectionError && <div className="launcher-connection-error" role="alert"><CircleAlert size={16} /><div><b>Firestorm could not connect</b><p>{connectionError}</p></div></div>}
      <div className="home-launcher-stats" aria-label="Session and snapshot status">
        <div><span>LOCAL SNAPSHOT</span><b>{hasSnapshot ? `${avatar.itemCount.toLocaleString()} items` : 'Not imported'}</b><small>{hasSnapshot ? `Updated ${snapshotTime}` : 'Choose one after Firestorm connects'}</small></div>
        <div><span>VIEWER SESSION</span><b>{connected ? 'Connected' : stockLeapStatus.phase === 'starting' ? 'Starting' : 'Not connected'}</b><small>{connected ? stockLeapAvatarLabel ?? 'Avatar detected' : 'Launch here to connect the viewer'}</small></div>
        <div><span>INVENTORY ACTIONS</span><b className={movesReady ? 'launcher-status-ready' : ''}>{moveStatus}</b><small>{movesReady ? 'Move selected items to Trash from Shelfmark' : connected ? 'Waiting for the matching session' : 'Connect to Firestorm first'}</small></div>
      </div>
    </section>

    <section className="home-shortcuts" aria-label="Continue in Shelfmark">
      <div className="home-shortcuts-heading"><span className="eyebrow">CONTINUE IN SHELFMARK</span><span>Local catalogue tools</span></div>
      <div className="home-shortcut-list">
        <button onClick={() => onNavigate('find')}><span><Search size={16} /><b>Find an item</b><small>Search names, notes, tags, and folder trails</small></span><ArrowRight size={15} /></button>
        <button onClick={() => onNavigate('folders')}><span><FolderTree size={16} /><b>Browse folders</b><small>Follow the inventory’s saved hierarchy</small></span><ArrowRight size={15} /></button>
        <button onClick={() => onNavigate('collections')}><span><Layers3 size={16} /><b>Outfit collections</b><small>Keep useful items together on this device</small></span><ArrowRight size={15} /></button>
      </div>
    </section>

    <p className="home-local-note"><ShieldCheck size={14} />Inventory snapshots and personal notes stay on this device.</p>
  </section>;
}

function SettingsPage({ desktop, avatar, summary, stockLeapStatus, stockLeapAvatarLabel, stockLeapBusy, stockLeapApiBusy, stockLeapApiDescription, stockLeapViewName, stockLeapViewBusy, stockLeapViewInspection, onStockLeapViewNameChange, onInspectStockLeapView, onDescribeStockLeapApi, onStartStockLeap, onInspectStockLeap, onScanStockLeapInventoryUi, onImport, onHelp, updateAvailable, updateVersion, updateStatus, updateBusy, onCheckUpdates, onInstallUpdate, profileBusy, onExportProfile, onImportProfile, protectedItemCount, protectedFolderCount, onClearProtection }: {
  desktop: boolean; avatar: AvatarOption; summary: ImportSummary | null; onImport: () => void; onHelp: () => void;
  stockLeapStatus: StockLeapStatus; stockLeapAvatarLabel: string | null; stockLeapBusy: boolean; stockLeapApiBusy: string | null; stockLeapApiDescription: StockLeapApiDescription | null; onDescribeStockLeapApi: (api: string) => void; onStartStockLeap: () => void; onInspectStockLeap: () => void; onScanStockLeapInventoryUi: () => void;
  stockLeapViewName: string; stockLeapViewBusy: boolean; stockLeapViewInspection: StockLeapViewInspection | null; onStockLeapViewNameChange: (name: string) => void; onInspectStockLeapView: () => void;
  updateAvailable: boolean; updateVersion: string | null; updateStatus: string; updateBusy: boolean; onCheckUpdates: () => void; onInstallUpdate: () => void;
  profileBusy: boolean; onExportProfile: () => void; onImportProfile: () => void;
  protectedItemCount: number; protectedFolderCount: number; onClearProtection: () => void;
}) {
  return <>
    <section className="page-heading settings-page-heading"><div><div className="eyebrow">YOUR WORKSPACE</div><h1>Settings</h1><p>Keep Shelfmark tidy, local, and up to date.</p></div></section>
    <section className="settings-list">
      <h2 className="settings-group-label">App</h2>
      <div className="settings-row"><div className="setting-mark"><CircleHelp size={17} /></div><div><b>How to use Shelfmark</b><p>Follow the short guide for importing a snapshot, finding items, and keeping local collections.</p></div><button className="quiet-button" onClick={onHelp}>Open the guide</button></div>
      <div className="settings-row"><div className="setting-mark"><RefreshCw size={17} /></div><div><b>Desktop app updates</b><p aria-live="polite">{desktop ? DEVELOPMENT_BUILD ? 'Update checks are available in signed release builds.' : updateStatus : 'Desktop installs update from signed releases; this browser updates when the site is redeployed.'}</p></div>{desktop ? <button className="quiet-button" onClick={updateAvailable ? onInstallUpdate : onCheckUpdates} disabled={updateBusy || DEVELOPMENT_BUILD} aria-label={updateAvailable ? `Install version ${updateVersion}` : 'Check for updates'}>{DEVELOPMENT_BUILD ? 'Preview build' : updateAvailable ? 'Install update' : updateBusy ? 'Checking…' : 'Check for updates'}</button> : <span className="setting-status muted-status">DESKTOP APP</span>}</div>
      <div className="settings-row"><div className="setting-mark"><Archive size={17} /></div><div><b>Personal data file</b><p>Export or restore your tags, notes, saved searches, avatar labels, collections, keep-safe marks, and photos. Inventory snapshots stay on this device.</p></div><div className="settings-actions"><button className="quiet-button" onClick={onExportProfile} disabled={profileBusy}><Download size={13} />{profileBusy ? 'Working…' : 'Export profile'}</button><button className="quiet-button" onClick={onImportProfile} disabled={profileBusy}>Restore profile</button></div></div>
      <h2 className="settings-group-label">Inventory</h2>
      <div className="settings-row"><div className="setting-mark"><FolderOpen size={17} /></div><div><b>{desktop ? 'Import a snapshot' : 'Desktop companion'}</b><p>{desktop ? 'Find Firestorm’s cache, choose an avatar snapshot, and confirm before Shelfmark reads it.' : 'Install the desktop companion to choose a local Firestorm snapshot.'}</p></div><button className="quiet-button" onClick={onImport}>{desktop ? 'Choose snapshot' : 'Desktop only'}</button></div>
      <div className="settings-row"><div className="setting-mark"><CloudOff size={17} /></div><div><b>Selected snapshot</b><p>{avatar.id === 'sample-avatar' ? 'No local snapshot selected.' : `${avatar.itemCount.toLocaleString()} items · imported ${relativeTime(avatar.snapshotAt ?? summary?.snapshotAt ?? '')}. Firestorm remains the source of truth.`}</p></div><span className="setting-status">{avatar.id === 'sample-avatar' ? 'EMPTY' : 'LOCAL'}</span></div>
      <div className="settings-row"><div className="setting-mark"><ShieldCheck size={17} /></div><div><b>Keep-safe marks</b><p>{protectedItemCount.toLocaleString()} items · {protectedFolderCount.toLocaleString()} folders. Local reminders only; Firestorm inventory is unchanged.</p></div><button className="quiet-button" onClick={onClearProtection} disabled={!protectedItemCount && !protectedFolderCount}>Clear marks</button></div>
      <h2 className="settings-group-label">Firestorm</h2>
      <div className="settings-row"><div className="setting-mark"><ExternalLink size={17} /></div><div><b>Viewer session</b><p>{desktop ? stockLeapStatus.connected ? `Connected${stockLeapAvatarLabel ? ` as ${stockLeapAvatarLabel}` : ''}. Shelfmark reads the active avatar automatically.` : 'Launch Firestorm from Home. Shelfmark reads the active avatar automatically.' : 'Live viewer actions are available in the desktop companion.'}</p></div><span className="setting-status">{desktop ? stockLeapStatus.connected ? 'CONNECTED' : 'AUTOMATIC' : 'DESKTOP'}</span></div>
      {desktop && DEVELOPMENT_BUILD && <details className="settings-dev-tools">
        <summary>Developer diagnostics</summary>
        <div className="settings-row stock-leap-row">
        <div className="setting-mark"><ExternalLink size={17} /></div>
        <div>
          <b>Stock Firestorm connection</b>
          <p>{desktop ? `${stockLeapStatus.message} ${stockLeapStatus.connected ? 'Shelfmark reads the active avatar and keeps viewer actions tied to the matching snapshot.' : 'Launch Firestorm from Home to load the helper automatically. Settings keeps the advanced connection diagnostics.'}` : 'The hosted browser cannot connect to a local viewer. Use the desktop companion for live inventory actions.'}</p>
          {stockLeapStatus.avatarId && <small className="stock-leap-details">Active avatar: {stockLeapAvatarLabel && <b>{stockLeapAvatarLabel} · </b>}<code>{stockLeapStatus.avatarId}</code> · {stockLeapStatus.apiNames.length} APIs read</small>}
          {desktop && DEVELOPMENT_BUILD && <details className="stock-leap-paths stock-leap-advanced">
            <summary>Advanced LEAP diagnostics</summary>
            {stockLeapStatus.inventoryPathsScanned && <details className="stock-leap-paths"><summary>Located inventory controls ({stockLeapStatus.inventoryWindowPaths.length})</summary><code>{stockLeapStatus.inventoryWindowPaths.slice(0, 100).join('\n')}</code></details>}
            {stockLeapStatus.connected && <button className="quiet-button stock-leap-scan-button" onClick={onScanStockLeapInventoryUi} disabled={stockLeapBusy || Boolean(stockLeapApiBusy)}>{stockLeapBusy ? 'Scanning…' : stockLeapStatus.inventoryPathsScanned ? 'Scan inventory UI again' : 'Scan inventory UI'}</button>}
            {stockLeapStatus.connected && <div className="stock-leap-lookup">
              <label htmlFor="stock-leap-view-name">Locate an inventory row by its exact name</label>
              <div><input id="stock-leap-view-name" type="text" maxLength={160} value={stockLeapViewName} onChange={(event) => onStockLeapViewNameChange(event.target.value)} placeholder="Test notecard name" /><button className="quiet-button" onClick={onInspectStockLeapView} disabled={!stockLeapViewName.trim() || stockLeapViewBusy || stockLeapBusy || Boolean(stockLeapApiBusy)}>{stockLeapViewBusy ? 'Locating…' : 'Locate item'}</button></div>
            </div>}
            {stockLeapViewInspection && <details className="stock-leap-paths stock-leap-api-result"><summary>Viewer match for {stockLeapViewInspection.name}</summary><pre>{stockLeapViewInspection.error ?? JSON.stringify(stockLeapViewInspection.result, null, 2)}</pre></details>}
            {stockLeapStatus.connected && stockLeapStatus.apiNames.length > 0 && <details className="stock-leap-paths stock-leap-apis">
              <summary>View {stockLeapStatus.apiNames.length} exposed APIs</summary>
              <ul>{stockLeapStatus.apiNames.map((api) => <li key={api}><code>{api}</code><button className="quiet-button" onClick={() => onDescribeStockLeapApi(api)} disabled={Boolean(stockLeapApiBusy) || stockLeapBusy}>{stockLeapApiBusy === api ? 'Reading…' : 'Read details'}</button></li>)}</ul>
            </details>}
            {stockLeapApiDescription && <details className="stock-leap-paths stock-leap-api-result"><summary>{stockLeapApiDescription.api} API details</summary><pre>{stockLeapApiDescription.error ?? (typeof stockLeapApiDescription.result === 'string' ? stockLeapApiDescription.result : JSON.stringify(stockLeapApiDescription.result, null, 2))}</pre></details>}
          </details>}
        </div>
        {desktop && DEVELOPMENT_BUILD ? <div className="settings-actions">
          {stockLeapStatus.connected ? <>
            <button className="quiet-button" onClick={onInspectStockLeap} disabled={stockLeapBusy || Boolean(stockLeapApiBusy)}>{stockLeapBusy ? 'Connecting…' : stockLeapStatus.avatarId ? 'Inspect again' : 'Inspect viewer'}</button>
          </> : <button className="quiet-button" onClick={onStartStockLeap} disabled={stockLeapBusy || stockLeapStatus.phase === 'starting'}>{stockLeapBusy || stockLeapStatus.phase === 'starting' ? 'Starting…' : 'Start stock Firestorm'}</button>}
        </div> : <span className="setting-status muted-status">DEVELOPMENT ONLY</span>}
        </div>
      </details>}
    </section>
    <div className="settings-footnote">{desktop ? 'Desktop companion' : 'Hosted browser'} <span>·</span> {avatar.id === 'sample-avatar' ? 'No inventory indexed' : 'Local inventory snapshot'}</div>
  </>;
}

function CacheDiscoveryDialog({ avatarLabels, avatarNames, candidates, viewerRunning, cacheDirectories, selectedCandidate, discoveryBusy, importing, progress, cancellationPending, error, onClose, onSelect, onImportSelected, onCancelImport, onBrowse, onRefresh }: {
  avatarLabels: Record<string, string>; avatarNames: Record<string, string>; candidates: CacheCandidate[]; viewerRunning: boolean | null; cacheDirectories: string[]; selectedCandidate: CacheCandidate | null; discoveryBusy: boolean; importing: boolean; progress: ImportProgress | null; cancellationPending: boolean; error: string;
  onClose: () => void; onSelect: (candidate: CacheCandidate) => void; onImportSelected: (candidate: CacheCandidate, avatarLabel: string) => void; onCancelImport: () => void; onBrowse: () => void; onRefresh: () => void;
}) {
  const dialogRef = useRef<HTMLElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const [filter, setFilter] = useState('');
  const [avatarLabelDraft, setAvatarLabelDraft] = useState<{ avatarId: string; value: string } | null>(null);
  const avatarLabel = selectedCandidate
    ? avatarLabelDraft?.avatarId === selectedCandidate.avatarId
      ? avatarLabelDraft.value
      : avatarLabels[selectedCandidate.avatarId] ?? ''
    : '';
  const localAvatarName = (avatarId: string) => avatarLabelDraft?.avatarId === avatarId
    ? avatarLabelDraft.value.trim() || avatarLabels[avatarId]?.trim() || avatarNames[avatarId]?.trim() || ''
    : avatarLabels[avatarId]?.trim() ?? avatarNames[avatarId]?.trim() ?? '';
  const filteredCandidates = candidates.filter((candidate) => `${localAvatarName(candidate.avatarId)} ${avatarNames[candidate.avatarId] ?? ''} ${candidate.avatarId} ${candidate.fileName}`.toLowerCase().includes(filter.trim().toLowerCase()));
  useDialogFocus(dialogRef, closeRef, onClose, !importing);

  return <div className="modal-scrim cache-scrim" onMouseDown={(event) => { if (event.target === event.currentTarget && !importing) onClose(); }}>
    <section ref={dialogRef} tabIndex={-1} className="cache-dialog" role="dialog" aria-modal="true" aria-labelledby="cache-dialog-title">
      <div className="cache-dialog-heading"><div><span className="eyebrow">LOCAL FIRESTORM FILES</span><h2 id="cache-dialog-title">Choose a snapshot.</h2></div><button ref={closeRef} className="icon-button" aria-label="Close cache chooser" onClick={onClose} disabled={importing}><X size={16} /></button></div>
      <p className="cache-dialog-intro">Choose the avatar snapshot to import. Shelfmark shows a saved local name or Firestorm’s cached avatar name when available. It reads only the matching name entry and file dates, and opens an inventory snapshot only after you confirm below.</p>
      <div className="cache-detection-status" role="status"><span className={`cache-detection-dot ${viewerRunning === true ? 'is-running' : ''}`} /><span><b>{discoveryBusy ? 'Checking Firestorm' : viewerRunning === true ? 'Firestorm process detected' : viewerRunning === false ? 'No Firestorm process detected' : 'Viewer status unavailable'}</b><small>{cacheDirectories.length ? <>Checking {cacheDirectories.map((directory, index) => <span key={directory}>{index ? ' · ' : ''}<code>{directory}</code></span>)}</> : 'Checking the standard Firestorm cache location'}<span>Launch Firestorm from Home to connect the stock viewer. The browser preview stays read-only.</span></small></span><button className="cache-rescan-button" onClick={onRefresh} disabled={discoveryBusy || importing}><RefreshCw size={12} /> Rescan</button></div>
      {error && <div className="cache-dialog-error" role="alert">{error}</div>}
      {importing && progress && <ImportProgressView progress={progress} cancellationPending={cancellationPending} />}
      {discoveryBusy ? <div className="cache-discovery-loading"><span className="spinner local-spinner" /> Checking Firestorm cache folders…</div> : candidates.length ? <>
        <div className="cache-list-heading"><span>{candidates.length} {candidates.length === 1 ? 'cache file' : 'cache files'} found</span><small>Select an avatar snapshot; confirm before it is read</small></div>
        <input className="cache-filter-input" value={filter} onChange={(event) => setFilter(event.target.value)} placeholder="Filter by avatar name, UUID or grid" aria-label="Filter detected cache files by avatar name, UUID or grid" />
        {selectedCandidate && <label className="avatar-label-field"><span>Local avatar name <small>Optional · stays on this device</small></span><input value={avatarLabel} maxLength={48} onChange={(event) => setAvatarLabelDraft({ avatarId: selectedCandidate.avatarId, value: event.target.value })} placeholder={avatarNames[selectedCandidate.avatarId] || 'For example, Salty Spicy'} aria-label="Optional local name for this avatar" /><code>{selectedCandidate.avatarId}</code></label>}
        <div className="cache-candidate-list">{filteredCandidates.map((candidate, index) => {
          const savedName = localAvatarName(candidate.avatarId);
          const visibleName = savedName || shortId(candidate.avatarId);
          const selected = selectedCandidate?.path === candidate.path;
          return <button className={`cache-candidate ${selected ? 'is-selected' : ''}`} key={`${candidate.path}-${index}`} onClick={() => onSelect(candidate)} disabled={importing} aria-pressed={selected} title={`${visibleName} · ${candidate.avatarId}`} aria-label={`${visibleName}; ${savedName ? `${shortId(candidate.avatarId)}; ` : ''}modified ${formatCacheModified(candidate.modifiedAtMs)}; ${selected ? 'selected' : 'select'}`}>
            <span className="cache-candidate-icon"><FolderOpen size={16} /></span><span className="cache-candidate-copy"><b>{visibleName}</b><small>{savedName ? `${shortId(candidate.avatarId)} · ` : ''}Modified {formatCacheModified(candidate.modifiedAtMs)} · {formatCacheSize(candidate.bytes)}</small></span><span className="cache-candidate-action">{selected ? 'Selected ✓' : 'Select'} </span>
          </button>;
        })}</div>
      </> : <div className="cache-empty"><FolderOpen size={19} /><b>No inventory caches found in the checked folders.</b><span>{cacheDirectories.length ? <>Shelfmark checked {cacheDirectories.map((directory, index) => <span key={directory}>{index ? ', ' : ''}<code>{directory}</code></span>)}. You can also browse directly to an inventory cache file or confirm Firestorm’s location in Preferences → Network & Files → Directories → Cache Location.</> : <>Browse to a file, or check Firestorm Preferences → Network & Files → Directories → Cache Location.</>}</span></div>}
      <div className="cache-dialog-footer"><span><ShieldCheck size={14} /> No cache is changed or uploaded.</span><div>{importing && <button className="quiet-button import-cancel-button" onClick={onCancelImport} disabled={cancellationPending}>{cancellationPending ? 'Stopping…' : 'Cancel import'}</button>}<button className="quiet-button" onClick={onClose} disabled={importing}>Cancel</button><button className="quiet-button" onClick={onBrowse} disabled={discoveryBusy || importing}><FolderOpen size={14} /> Browse</button><button className="primary-action" onClick={() => selectedCandidate && onImportSelected(selectedCandidate, avatarLabel)} disabled={!selectedCandidate || importing}><FolderOpen size={14} /> {importing ? 'Importing…' : 'Import selected avatar'}</button></div></div>
    </section>
  </div>;
}

function ImportProgressView({ progress, cancellationPending }: { progress: ImportProgress; cancellationPending: boolean }) {
  const labels: Record<string, string> = {
    reading_cache: 'Reading the selected cache',
    decompressing: 'Expanding compressed cache data',
    cache_ready: 'Preparing inventory metadata',
    decoding: 'Decoding inventory metadata',
    reading_folders: 'Reading folders',
    reading_items: 'Reading inventory items',
    building_paths: 'Building folder paths',
    scoring_items: 'Preparing search index',
    indexing_folders: 'Indexing folders',
    indexing_items: 'Indexing inventory items',
    finishing: 'Finishing the local catalogue',
  };
  const percent = progress.total && progress.total > 0
    ? Math.min(100, Math.floor(progress.completed / progress.total * 100))
    : null;
  const label = cancellationPending ? 'Stopping safely…' : labels[progress.stage] ?? 'Importing snapshot';
  const count = progress.total && progress.total > 0
    ? `${progress.completed.toLocaleString()} of ${progress.total.toLocaleString()}${percent === null ? '' : ` · ${percent}%`}`
    : progress.completed > 0 ? `${progress.completed.toLocaleString()} processed` : 'Working locally';
  return <div className="import-progress" role="status" aria-live="polite">
    <div className="import-progress-copy"><b>{label}</b><span>{count}</span></div>
    {progress.total && progress.total > 0
      ? <progress className="import-progress-track" max={progress.total} value={Math.min(progress.completed, progress.total)} aria-label={`${label}: ${percent}%`} />
      : <progress className="import-progress-track" aria-label={label} />}
    <small>Cancel keeps the existing avatar catalogue intact.</small>
  </div>;
}

function formatCacheModified(timestamp: number): string {
  if (!timestamp) return 'date unavailable';
  return new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' }).format(new Date(timestamp));
}

function formatCacheSize(bytes: number): string {
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function UpdateProgressDialog({ status }: { status: string }) {
  const dialogRef = useRef<HTMLElement>(null);
  useDialogFocus(dialogRef, dialogRef, () => undefined, false);
  return <div className="modal-scrim"><section ref={dialogRef} tabIndex={-1} className="cache-dialog update-progress-dialog" role="dialog" aria-modal="true" aria-labelledby="update-progress-title">
    <span className="eyebrow">SHELFMARK UPDATE</span><h2 id="update-progress-title">Updating Shelfmark</h2>
    <p className="cache-dialog-intro" role="status" aria-live="polite">{status}</p><p>Keep Shelfmark open. It will restart when installation is complete.</p>
  </section></div>;
}

function GuideDialog({ desktop, onClose, onImport }: { desktop: boolean; onClose: () => void; onImport: () => void }) {
  const dialogRef = useRef<HTMLElement>(null);
  const closeButtonRef = useRef<HTMLButtonElement>(null);
  useDialogFocus(dialogRef, closeButtonRef, onClose, true);
  return <div className="modal-scrim guide-scrim" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}>
    <section ref={dialogRef} tabIndex={-1} className="guide-modal" role="dialog" aria-modal="true" aria-labelledby="guide-title">
      <div className="guide-heading"><div><span className="eyebrow">A SHORT FIELD GUIDE</span><h2 id="guide-title">Find the thing you half remember.</h2><p>{desktop ? 'Search a local Firestorm snapshot, then use Firestorm for inventory changes.' : 'Explore a fictional sample catalogue. The browser preview cannot see Firestorm or change live inventory.'}</p></div><button ref={closeButtonRef} className="icon-button" aria-label="Close guide" onClick={onClose}><X size={17} /></button></div>
      <div className="guide-steps">
        <article><span className="guide-number">01</span><div><h3>{desktop ? 'Choose a Firestorm snapshot' : 'Explore the sample first'}</h3><p>{desktop ? 'Open Import Firestorm cache, choose a detected avatar snapshot (or browse to one), give it an optional local name, and confirm. Shelfmark reads the selected cache only after you confirm.' : 'The browser preview uses fictional inventory. It cannot read files from Firestorm; use the desktop app to import a local snapshot.'}</p></div></article>
        <article><span className="guide-number">02</span><div><h3>Search, or browse the folder tree</h3><p>Find items by name, path, description, or your tags. Open Folders to follow the cache hierarchy with breadcrumbs, then choose an item to inspect it. The desktop app can select a snapshot entry in the registered viewer when the matching avatar and grid are logged in. Use / or Ctrl/⌘ K to focus the active search.</p></div></article>
        <article><span className="guide-number">03</span><div><h3>Add your own context</h3><p>Add personal tags and notes, save items into collections, and choose reference photos or collection covers. Images stay in this app profile on this device; Shelfmark never uploads them.</p></div></article>
        <article><span className="guide-number">04</span><div><h3>{desktop ? 'Send items to Trash on Windows' : 'Use the desktop app for live inventory'}</h3><p>{desktop ? 'On Windows, close Firestorm first, then launch it from Home and select the matching Second Life snapshot. Keep All Items open and clear the inventory search. Choose Move to Trash for one selected item; Shelfmark checks the new location. Close the viewer and refresh the saved snapshot afterward. Live Firestorm actions are not currently available on macOS or Linux.' : 'A browser cannot open Firestorm or read its cache. It stays on fictional sample data; use the installed desktop app to import a local snapshot. Live Firestorm actions are currently available on Windows only.'}</p></div></article>
      </div>
      <div className="guide-note"><ShieldCheck size={16} /><p><b>Keep the snapshot in context:</b> “Recently added” uses a cached inventory timestamp, not purchase history. Candidate groups are clues only. A snapshot can be stale or incomplete. Refresh it after Firestorm saves changes.</p></div>
      <div className="guide-footer"><span>{desktop ? 'Local index · No SL password · Windows live Trash' : 'Fictional sample · No SL password · Browser read-only'}</span><div><button className="quiet-button" onClick={onClose}>Close guide</button>{desktop && <button className="primary-action" onClick={onImport}><FolderOpen size={14} /> Choose a cache</button>}</div></div>
    </section>
  </div>;
}

function ProfileRestoreDialog({ backup, busy, onCancel, onRestore }: {
  backup: ProfileBackup; busy: boolean; onCancel: () => void; onRestore: () => void;
}) {
  const dialogRef = useRef<HTMLElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const { avatarCount, keepSafeCount, photoBytes } = useMemo(() => {
    const profiles = Object.entries(backup.localStorage).filter(([key]) => key !== 'shelfmark:avatar-labels:v1');
    return {
      avatarCount: profiles.length,
      keepSafeCount: profiles.reduce((count, [, value]) => {
        const profile = JSON.parse(value) as LocalPreferences;
        return count + profile.protectedItemIds.length + profile.protectedFolderIds.length;
      }, 0),
      photoBytes: backup.images.reduce((total, image) => total + image.bytes, 0),
    };
  }, [backup]);
  useDialogFocus(dialogRef, cancelRef, onCancel, !busy);

  return <div className="modal-scrim guide-scrim" onMouseDown={(event) => { if (event.target === event.currentTarget && !busy) onCancel(); }}>
    <section ref={dialogRef} tabIndex={-1} className="guide-modal profile-restore-dialog" role="dialog" aria-modal="true" aria-labelledby="profile-restore-title">
      <div className="guide-heading"><div><span className="eyebrow">RESTORE A LOCAL PROFILE</span><h2 id="profile-restore-title">Replace this device’s personal setup?</h2><p>This file is from {new Date(backup.exportedAt).toLocaleString()}. It contains your private Shelfmark customizations and photos.</p></div><button ref={cancelRef} className="icon-button" aria-label="Cancel profile restore" onClick={onCancel} disabled={busy}><X size={17} /></button></div>
      <div className="profile-restore-summary"><div><b>{avatarCount.toLocaleString()}</b><span>avatar profiles</span></div><div><b>{keepSafeCount.toLocaleString()}</b><span>keep-safe marks</span></div><div><b>{backup.images.length.toLocaleString()}</b><span>local photos</span></div><div><b>{(photoBytes / (1024 * 1024)).toFixed(1)} MB</b><span>photo data</span></div></div>
      <div className="guide-note"><ShieldCheck size={16} /><p><b>Only personal Shelfmark data is replaced.</b> This restore does not include or alter the inventory index or Firestorm files. On a new device, import the matching Firestorm cache after restoring to reconnect notes, tags, and photos to items.</p></div>
      <div className="guide-footer"><span>Nothing is sent to a server</span><div><button className="quiet-button" onClick={onCancel} disabled={busy}>Cancel</button><button className="primary-action" onClick={onRestore} disabled={busy}><Archive size={14} />{busy ? 'Restoring…' : 'Replace personal data'}</button></div></div>
    </section>
  </div>;
}

export default App;
