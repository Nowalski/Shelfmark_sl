import type { LocalPreferences, SavedSearch } from './types';

const kinds = new Set(['Any type', 'Clothing', 'Object', 'Notecard', 'Landmark', 'Folder', 'Other']);
const record = (value: unknown): value is Record<string, unknown> => Boolean(value) && typeof value === 'object' && !Array.isArray(value);
const validId = (value: unknown): value is string => typeof value === 'string' && value.length > 0 && value.length <= 256;
const ids = (value: unknown) => Array.isArray(value) ? [...new Set(value.filter(validId))].slice(0, 100_000) : [];

// Local preferences may come from older builds or an interrupted/manual edit.
// Recover each section independently so one bad entry cannot crash the UI or
// discard unrelated notes, tags, and collections.
export function normalizeLocalPreferences(value: unknown, defaults: LocalPreferences): LocalPreferences {
  const stored = record(value) ? value : {};
  const savedSearches = (Array.isArray(stored.savedSearches) ? stored.savedSearches : defaults.savedSearches)
    .slice(0, 20).flatMap((entry): SavedSearch[] => {
      if (!record(entry) || typeof entry.label !== 'string' || !entry.label.trim() || typeof entry.query !== 'string') return [];
      const legacyRecent = entry.label === 'Recent arrivals' && entry.query.trim() === '';
      return [{
        label: legacyRecent ? 'Recently added' : entry.label.trim().slice(0, 80),
        query: entry.query.slice(0, 300),
        kind: typeof entry.kind === 'string' && kinds.has(entry.kind) ? entry.kind as SavedSearch['kind'] : 'Any type',
        page: entry.page === 'new' || (entry.page !== 'find' && legacyRecent) ? 'new' : 'find',
      }];
    });
  const itemTags = Object.fromEntries((record(stored.itemTags) ? Object.entries(stored.itemTags) : [])
    .filter(([id, tags]) => validId(id) && Array.isArray(tags)).slice(0, 100_000)
    .map(([id, tags]) => [id, [...new Set((tags as unknown[]).filter((tag): tag is string => typeof tag === 'string')
      .map((tag) => tag.trim().slice(0, 40)).filter(Boolean))].slice(0, 16)]));
  const itemNotes = Object.fromEntries((record(stored.itemNotes) ? Object.entries(stored.itemNotes) : [])
    .filter(([id, note]) => validId(id) && typeof note === 'string').slice(0, 100_000)
    .map(([id, note]) => [id, (note as string).slice(0, 5_000)]));
  const seenCollections = new Set<string>();
  const collections = (Array.isArray(stored.collections) ? stored.collections : []).slice(0, 1_000)
    .flatMap((entry): LocalPreferences['collections'] => {
      if (!record(entry) || !validId(entry.id) || entry.id.length > 128 || seenCollections.has(entry.id)
        || typeof entry.name !== 'string' || !entry.name.trim()) return [];
      seenCollections.add(entry.id);
      return [{ id: entry.id, name: entry.name.trim().slice(0, 100), itemIds: ids(entry.itemIds) }];
    });
  return {
    savedSearches, itemTags, itemNotes, collections,
    protectedItemIds: ids(stored.protectedItemIds), protectedFolderIds: ids(stored.protectedFolderIds),
    theme: stored.theme === 'dark' ? 'dark' : defaults.theme,
  };
}
