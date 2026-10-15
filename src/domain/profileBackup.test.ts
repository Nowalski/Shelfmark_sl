import { describe, expect, it } from 'vitest';
import {
  avatarTagsFromBackup,
  createProfileBackup,
  decodeProfileImage,
  parseProfileBackup,
  PROFILE_LABELS_KEY,
  replaceProfileStorage,
} from './profileBackup';

class MemoryStorage {
  private values = new Map<string, string>();
  get length() { return this.values.size; }
  key(index: number) { return Array.from(this.values.keys())[index] ?? null; }
  getItem(key: string) { return this.values.get(key) ?? null; }
  setItem(key: string, value: string) { this.values.set(key, String(value)); }
  removeItem(key: string) { this.values.delete(key); }
}

const preferences = {
  savedSearches: [
    { label: 'Winter layers', query: 'jacket', kind: 'Clothing', page: 'find' },
    { label: 'Recently added', query: '', kind: 'Any type', page: 'new' },
  ],
  itemTags: { 'item-1': ['legacy', 'wool'] },
  itemNotes: { 'item-1': 'Keep this with the boots.' },
  protectedItemIds: ['item-1'],
  protectedFolderIds: ['folder-1'],
  collections: [{ id: 'winter-board', name: 'Winter outfits', itemIds: ['item-1', 'item-2'] }],
  theme: 'dark',
};

describe('Shelfmark profile files', () => {
  it('round-trips saved preferences, avatar labels, and user-selected photos', async () => {
    const storage = new MemoryStorage();
    storage.setItem('shelfmark:avatar-1@secondlife', JSON.stringify(preferences));
    storage.setItem(PROFILE_LABELS_KEY, JSON.stringify({ 'avatar-1@secondlife': 'Salty Spicy' }));
    storage.setItem('shelfmark:sample-avatar', JSON.stringify({ sample: true }));
    storage.setItem('shelfmark:sample-seed-complete', 'true');
    const image = new Blob([new Uint8Array([255, 216, 255, 217])], { type: 'image/jpeg' });

    const serialized = await createProfileBackup(storage, [{ key: 'avatar-1@secondlife:item:item-1', image }], '2026-09-29T12:00:00.000Z');
    const backup = parseProfileBackup(serialized);

    expect(Object.keys(backup.localStorage).sort()).toEqual(['shelfmark:avatar-1@secondlife', PROFILE_LABELS_KEY].sort());
    expect(avatarTagsFromBackup(backup)).toEqual([{ avatarId: 'avatar-1@secondlife', itemTags: { 'item-1': ['legacy', 'wool'] } }]);
    expect(JSON.parse(backup.localStorage['shelfmark:avatar-1@secondlife']).protectedItemIds).toEqual(['item-1']);
    expect(JSON.parse(backup.localStorage['shelfmark:avatar-1@secondlife']).protectedFolderIds).toEqual(['folder-1']);
    expect(JSON.parse(backup.localStorage['shelfmark:avatar-1@secondlife']).savedSearches).toEqual(preferences.savedSearches);
    expect(backup.images).toHaveLength(1);
    expect(new Uint8Array(await decodeProfileImage(backup.images[0]).arrayBuffer())).toEqual(new Uint8Array([255, 216, 255, 217]));
  });

  it('replaces personal preferences while keeping the fictional sample profile', () => {
    const storage = new MemoryStorage();
    storage.setItem('shelfmark:old-avatar', JSON.stringify(preferences));
    storage.setItem('shelfmark:sample-avatar', JSON.stringify({ sample: true }));
    storage.setItem('shelfmark:sample-seed-complete', 'true');
    replaceProfileStorage(storage, { 'shelfmark:new-avatar': JSON.stringify(preferences) });

    expect(storage.getItem('shelfmark:old-avatar')).toBeNull();
    expect(storage.getItem('shelfmark:new-avatar')).not.toBeNull();
    expect(storage.getItem('shelfmark:sample-avatar')).toContain('sample');
    expect(storage.getItem('shelfmark:sample-seed-complete')).toBe('true');
  });

  it('restores legacy profiles without keep-safe fields and rejects malformed marks', async () => {
    const storage = new MemoryStorage();
    storage.setItem('shelfmark:avatar-1', JSON.stringify(preferences));
    const raw = JSON.parse(await createProfileBackup(storage, [])) as { localStorage: Record<string, string> };
    const avatarPreferences = JSON.parse(raw.localStorage['shelfmark:avatar-1']) as Record<string, unknown>;
    delete avatarPreferences.protectedItemIds;
    delete avatarPreferences.protectedFolderIds;
    raw.localStorage['shelfmark:avatar-1'] = JSON.stringify(avatarPreferences);
    const restored = parseProfileBackup(JSON.stringify(raw));
    const restoredPreferences = JSON.parse(restored.localStorage['shelfmark:avatar-1']);
    expect(restoredPreferences.protectedItemIds).toEqual([]);
    expect(restoredPreferences.protectedFolderIds).toEqual([]);

    avatarPreferences.protectedItemIds = [null];
    raw.localStorage['shelfmark:avatar-1'] = JSON.stringify(avatarPreferences);
    expect(() => parseProfileBackup(JSON.stringify(raw))).toThrow('keep-safe item list is malformed');
  });

  it('migrates the old empty Recent arrivals shortcut to the dated-items view', async () => {
    const storage = new MemoryStorage();
    storage.setItem('shelfmark:avatar-1', JSON.stringify({
      ...preferences,
      savedSearches: [{ label: 'Recent arrivals', query: '', kind: 'Any type' }],
    }));

    const backup = parseProfileBackup(await createProfileBackup(storage, []));
    const restored = JSON.parse(backup.localStorage['shelfmark:avatar-1']);
    expect(restored.savedSearches).toEqual([{ label: 'Recently added', query: '', kind: 'Any type', page: 'new' }]);
  });

  it('rejects unsupported formats, invalid preferences, and executable image types', async () => {
    const storage = new MemoryStorage();
    storage.setItem('shelfmark:avatar-1', JSON.stringify(preferences));
    await expect(createProfileBackup(storage, [{ key: 'avatar-1:item:item-1', image: new Blob(['<svg/>'], { type: 'image/svg+xml' }) }])).rejects.toThrow('unsupported type');
    expect(() => parseProfileBackup(JSON.stringify({ format: 'other', version: 1, exportedAt: '2026-09-29', localStorage: {}, images: [] }))).toThrow('not a supported Shelfmark profile');
    const malformed = await createProfileBackup(storage, []);
    const malformedData = JSON.parse(malformed) as { localStorage: Record<string, string> };
    const avatarKey = Object.keys(malformedData.localStorage)[0];
    const malformedPreferences = JSON.parse(malformedData.localStorage[avatarKey]) as { theme: string };
    malformedPreferences.theme = 'solar';
    malformedData.localStorage[avatarKey] = JSON.stringify(malformedPreferences);
    expect(() => parseProfileBackup(JSON.stringify(malformedData))).toThrow('saved theme is malformed');
  });
});
