import type { LocalPreferences, SavedSearch } from './types';

export const PROFILE_BACKUP_FORMAT = 'shelfmark-profile';
export const PROFILE_BACKUP_VERSION = 1;
export const PROFILE_LABELS_KEY = 'shelfmark:avatar-labels:v1';
export const MAX_PROFILE_MEDIA_BYTES = 64 * 1024 * 1024;
export const MAX_PROFILE_FILE_CHARS = 128 * 1024 * 1024;
const MAX_PREFERENCE_BYTES = 16 * 1024 * 1024;
const MAX_IMAGE_BYTES = 8 * 1024 * 1024;
const MAX_MEDIA_COUNT = 20_000;
const MAX_PROFILE_COUNT = 1_000;
const MAX_AVATAR_LABEL_LENGTH = 48;
const IMAGE_TYPES = new Set(['image/jpeg', 'image/png', 'image/webp']);
const SEARCH_KINDS = new Set(['Any type', 'Clothing', 'Object', 'Notecard', 'Landmark', 'Folder', 'Other']);

export type ProfileImageSource = { key: string; image: Blob };
export type ProfileImageBackup = { key: string; mimeType: string; bytes: number; dataBase64: string };
export type ProfileBackup = {
  format: typeof PROFILE_BACKUP_FORMAT;
  version: typeof PROFILE_BACKUP_VERSION;
  exportedAt: string;
  localStorage: Record<string, string>;
  images: ProfileImageBackup[];
};
export type AvatarTagBackup = { avatarId: string; itemTags: Record<string, string[]> };

type StorageLike = Pick<Storage, 'length' | 'key' | 'getItem'>;
export type ProfileStorage = StorageLike & Pick<Storage, 'setItem' | 'removeItem'>;

export function withProfileStorageOverrides(storage: StorageLike, overrides: Record<string, string>): StorageLike {
  // Capture values before asynchronous photo reads or a native save dialog.
  // A backup must not mix settings from different moments in the export.
  const values = new Map<string, string>();
  for (let index = 0; index < storage.length; index += 1) {
    const key = storage.key(index);
    if (key?.startsWith('shelfmark:')) {
      const value = storage.getItem(key);
      if (value !== null) values.set(key, value);
    }
  }
  for (const [key, value] of Object.entries(overrides)) values.set(key, value);
  const orderedKeys = [...values.keys()];
  return {
    length: orderedKeys.length,
    key: (index) => orderedKeys[index] ?? null,
    getItem: (key) => values.get(key) ?? null,
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function boundedString(value: unknown, maximum: number): value is string {
  return typeof value === 'string' && value.length <= maximum;
}

function parseJson(value: string, label: string): unknown {
  try { return JSON.parse(value) as unknown; }
  catch { throw new Error(`${label} is not valid JSON.`); }
}

function normalizeIdList(value: unknown, label: string): string[] {
  if (value === undefined) return [];
  if (!Array.isArray(value) || value.length > 100_000
    || !value.every((id) => typeof id === 'string' && id.length > 0 && id.length <= 256)) {
    throw new Error(`A ${label} list is malformed.`);
  }
  return Array.from(new Set(value as string[]));
}

function normalizePreferences(value: unknown): LocalPreferences {
  if (!isRecord(value)) throw new Error('A saved Shelfmark profile is malformed.');
  if (!Array.isArray(value.savedSearches) || value.savedSearches.length > 20) throw new Error('A saved search list is malformed.');
  if (!isRecord(value.itemTags) || Object.keys(value.itemTags).length > 100_000) throw new Error('A personal tag list is malformed.');
  if (!isRecord(value.itemNotes) || Object.keys(value.itemNotes).length > 100_000) throw new Error('A personal note list is malformed.');
  if (!Array.isArray(value.collections) || value.collections.length > 1_000) throw new Error('A collection list is malformed.');
  if (value.theme !== 'light' && value.theme !== 'dark') throw new Error('A saved theme is malformed.');

  const savedSearches: SavedSearch[] = value.savedSearches.map((entry) => {
    if (!isRecord(entry) || !boundedString(entry.label, 80) || !entry.label.trim() || !boundedString(entry.query, 300)) {
      throw new Error('A saved search contains invalid text.');
    }
    const kind = entry.kind ?? 'Any type';
    if (typeof kind !== 'string' || !SEARCH_KINDS.has(kind)) throw new Error('A saved search has an unknown item type.');
    const legacyRecent = entry.label === 'Recent arrivals' && entry.query.trim() === '';
    const page = entry.page ?? (legacyRecent ? 'new' : 'find');
    if (page !== 'find' && page !== 'new') throw new Error('A saved search has an unknown catalogue view.');
    return {
      label: legacyRecent ? 'Recently added' : entry.label.trim(),
      query: entry.query,
      kind: kind as SavedSearch['kind'],
      page,
    };
  });

  const itemTags: LocalPreferences['itemTags'] = {};
  for (const [itemId, tags] of Object.entries(value.itemTags)) {
    if (!itemId || itemId.length > 256 || !Array.isArray(tags) || tags.length > 16 || !tags.every((tag) => boundedString(tag, 40))) {
      throw new Error('A personal tag entry is malformed.');
    }
    itemTags[itemId] = Array.from(new Set(tags.map((tag) => tag.trim().slice(0, 40)).filter(Boolean)));
  }

  const itemNotes: LocalPreferences['itemNotes'] = {};
  for (const [itemId, note] of Object.entries(value.itemNotes)) {
    if (!itemId || itemId.length > 256 || !boundedString(note, 5_000)) throw new Error('A personal note entry is malformed.');
    itemNotes[itemId] = note;
  }

  const protectedItemIds = normalizeIdList(value.protectedItemIds, 'keep-safe item');
  const protectedFolderIds = normalizeIdList(value.protectedFolderIds, 'keep-safe folder');

  const collections = value.collections.map((entry) => {
    if (!isRecord(entry) || !boundedString(entry.id, 128) || !entry.id || !boundedString(entry.name, 100) || !entry.name.trim()
      || !Array.isArray(entry.itemIds) || entry.itemIds.length > 100_000
      || !entry.itemIds.every((itemId) => boundedString(itemId, 256) && itemId.length > 0)) {
      throw new Error('A collection entry is malformed.');
    }
    return { id: entry.id, name: entry.name.trim(), itemIds: Array.from(new Set(entry.itemIds as string[])) };
  });

  return { savedSearches, itemTags, itemNotes, protectedItemIds, protectedFolderIds, collections, theme: value.theme };
}

export function collectProfileStorage(storage: StorageLike): Record<string, string> {
  const output: Record<string, string> = {};
  for (let index = 0; index < storage.length; index += 1) {
    const key = storage.key(index);
    if (!key || !key.startsWith('shelfmark:')) continue;
    const value = storage.getItem(key);
    if (value === null) continue;
    if (key === PROFILE_LABELS_KEY) {
      const parsed = parseJson(value, 'Avatar names');
      if (!isRecord(parsed) || Object.keys(parsed).length > MAX_PROFILE_COUNT) throw new Error('Saved avatar names are malformed.');
      const labels: Record<string, string> = {};
      for (const [avatarId, label] of Object.entries(parsed)) {
        if (!avatarId || avatarId.length > 256 || !boundedString(label, 256)) throw new Error('A saved avatar name is malformed.');
        const normalized = label.trim().slice(0, MAX_AVATAR_LABEL_LENGTH);
        if (normalized) labels[avatarId] = normalized;
      }
      output[key] = JSON.stringify(labels);
      continue;
    }

    const avatarId = key.slice('shelfmark:'.length);
    if (!avatarId || avatarId === 'sample-seed-complete' || avatarId === 'sample-avatar' || avatarId.startsWith('sample-')) continue;
    if (avatarId.length > 256) throw new Error('A saved avatar profile key is malformed.');
    const preferences = normalizePreferences(parseJson(value, 'Avatar preferences'));
    output[key] = JSON.stringify(preferences);
  }
  if (Object.keys(output).length > MAX_PROFILE_COUNT + 1) throw new Error('This device has too many saved avatar profiles to export.');
  if (JSON.stringify(output).length > MAX_PREFERENCE_BYTES) throw new Error('Your personal notes are too large for one profile export.');
  return output;
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = '';
  const chunkSize = 0x8000;
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(offset, Math.min(offset + chunkSize, bytes.length)));
  }
  return btoa(binary);
}

function decodedLength(data: string): number {
  if (!data || data.length % 4 !== 0 || !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(data)) {
    throw new Error('A photo in this profile file is not valid base64 data.');
  }
  const padding = data.endsWith('==') ? 2 : data.endsWith('=') ? 1 : 0;
  return (data.length / 4) * 3 - padding;
}

export async function createProfileBackup(
  storage: StorageLike,
  media: ProfileImageSource[],
  exportedAt = new Date().toISOString(),
): Promise<string> {
  if (!Number.isFinite(Date.parse(exportedAt))) throw new Error('The export timestamp is invalid.');
  if (media.length > MAX_MEDIA_COUNT) throw new Error('There are too many saved photos for one profile export.');
  const localStorage = collectProfileStorage(storage);
  const keys = new Set<string>();
  let totalBytes = 0;
  const images: ProfileImageBackup[] = [];
  for (const { key, image } of media) {
    if (!key || key.length > 512 || keys.has(key)) throw new Error('A saved photo key is malformed or duplicated.');
    keys.add(key);
    if (!IMAGE_TYPES.has(image.type) || image.size === 0 || image.size > MAX_IMAGE_BYTES) throw new Error('A saved photo has an unsupported type or size.');
    totalBytes += image.size;
    if (totalBytes > MAX_PROFILE_MEDIA_BYTES) throw new Error('Saved photos total more than 64 MB. Remove a few large pictures and try again.');
    images.push({ key, mimeType: image.type, bytes: image.size, dataBase64: bytesToBase64(new Uint8Array(await image.arrayBuffer())) });
  }

  const backup: ProfileBackup = {
    format: PROFILE_BACKUP_FORMAT,
    version: PROFILE_BACKUP_VERSION,
    exportedAt,
    localStorage,
    images,
  };
  const serialized = JSON.stringify(backup);
  if (serialized.length > MAX_PROFILE_FILE_CHARS) throw new Error('The profile export is too large to keep in one file.');
  return serialized;
}

export function parseProfileBackup(serialized: string): ProfileBackup {
  if (serialized.length > MAX_PROFILE_FILE_CHARS) throw new Error('This profile file is larger than the supported 128 MB limit.');
  const parsed = parseJson(serialized, 'This Shelfmark profile file');
  if (!isRecord(parsed) || parsed.format !== PROFILE_BACKUP_FORMAT || parsed.version !== PROFILE_BACKUP_VERSION
    || !boundedString(parsed.exportedAt, 64) || !Number.isFinite(Date.parse(parsed.exportedAt))) {
    throw new Error('This is not a supported Shelfmark profile file.');
  }
  if (!isRecord(parsed.localStorage) || !Array.isArray(parsed.images) || parsed.images.length > MAX_MEDIA_COUNT) {
    throw new Error('This Shelfmark profile file is incomplete.');
  }

  const localStorage: Record<string, string> = {};
  for (const [key, value] of Object.entries(parsed.localStorage)) {
    if (!key.startsWith('shelfmark:') || !boundedString(value, MAX_PREFERENCE_BYTES)) throw new Error('This profile includes an invalid local preference.');
    if (key === PROFILE_LABELS_KEY) {
      const labels = parseJson(value, 'Avatar names');
      if (!isRecord(labels) || Object.keys(labels).length > MAX_PROFILE_COUNT
        || !Object.entries(labels).every(([id, label]) => Boolean(id) && id.length <= 256 && boundedString(label, MAX_AVATAR_LABEL_LENGTH))) {
        throw new Error('This profile includes invalid avatar names.');
      }
      localStorage[key] = JSON.stringify(labels);
      continue;
    }
    const avatarId = key.slice('shelfmark:'.length);
    if (!avatarId || avatarId.length > 256 || avatarId === 'sample-seed-complete' || avatarId === 'sample-avatar' || avatarId.startsWith('sample-')) {
      throw new Error('This profile includes an invalid avatar key.');
    }
    localStorage[key] = JSON.stringify(normalizePreferences(parseJson(value, 'Avatar preferences')));
  }
  if (Object.keys(localStorage).length > MAX_PROFILE_COUNT + 1 || JSON.stringify(localStorage).length > MAX_PREFERENCE_BYTES) {
    throw new Error('The local preferences in this profile file are too large.');
  }

  const keys = new Set<string>();
  let totalBytes = 0;
  const images: ProfileImageBackup[] = parsed.images.map((value) => {
    if (!isRecord(value) || !boundedString(value.key, 512) || !value.key || keys.has(value.key)
      || typeof value.mimeType !== 'string' || !IMAGE_TYPES.has(value.mimeType)
      || !boundedString(value.dataBase64, MAX_PROFILE_FILE_CHARS)
      || !Number.isSafeInteger(value.bytes) || (value.bytes as number) <= 0 || (value.bytes as number) > MAX_IMAGE_BYTES) {
      throw new Error('This profile contains an invalid photo.');
    }
    keys.add(value.key);
    const actualBytes = decodedLength(value.dataBase64);
    if (actualBytes !== value.bytes) throw new Error('A photo in this profile file is incomplete.');
    totalBytes += actualBytes;
    if (totalBytes > MAX_PROFILE_MEDIA_BYTES) throw new Error('Photos in this profile file total more than 64 MB.');
    return { key: value.key, mimeType: value.mimeType, bytes: actualBytes, dataBase64: value.dataBase64 };
  });

  return {
    format: PROFILE_BACKUP_FORMAT,
    version: PROFILE_BACKUP_VERSION,
    exportedAt: parsed.exportedAt,
    localStorage,
    images,
  };
}

export function avatarTagsFromBackup(backup: ProfileBackup): AvatarTagBackup[] {
  return Object.entries(backup.localStorage)
    .filter(([key]) => key !== PROFILE_LABELS_KEY)
    .map(([key, value]) => ({ avatarId: key.slice('shelfmark:'.length), itemTags: normalizePreferences(JSON.parse(value) as unknown).itemTags }));
}

export function replaceProfileStorage(storage: ProfileStorage, values: Record<string, string>): void {
  const previous: Record<string, string> = {};
  for (let index = 0; index < storage.length; index += 1) {
    const key = storage.key(index);
    if (!key || !key.startsWith('shelfmark:') || key.slice('shelfmark:'.length).startsWith('sample-')) continue;
    const value = storage.getItem(key);
    if (value !== null) previous[key] = value;
  }
  const nextKeys = Object.keys(values);
  try {
    for (const key of Object.keys(previous)) storage.removeItem(key);
    for (const [key, value] of Object.entries(values)) storage.setItem(key, value);
  } catch {
    for (const key of [...Object.keys(previous), ...nextKeys]) storage.removeItem(key);
    for (const [key, value] of Object.entries(previous)) storage.setItem(key, value);
    throw new Error('Could not restore local profile settings. Your previous settings were kept.');
  }
}

export function decodeProfileImage(image: ProfileImageBackup): Blob {
  const binary = atob(image.dataBase64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  return new Blob([bytes], { type: image.mimeType });
}
