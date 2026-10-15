import { describe, expect, it } from 'vitest';
import { MAX_AVATAR_LABEL_LENGTH, readAvatarLabels, saveAvatarLabel } from './avatarLabels';

function memoryStorage(): Storage {
  const values = new Map<string, string>();
  return {
    get length() { return values.size; },
    clear: () => values.clear(),
    getItem: (key) => values.get(key) ?? null,
    key: (index) => Array.from(values.keys())[index] ?? null,
    removeItem: (key) => { values.delete(key); },
    setItem: (key, value) => { values.set(key, String(value)); },
  };
}

describe('local avatar labels', () => {
  it('stores a trimmed label per avatar and keeps labels isolated', () => {
    const storage = memoryStorage();
    saveAvatarLabel('avatar-a@secondlife', '  Salty Spicy  ', storage);
    saveAvatarLabel('avatar-b@grid_test', 'Alt account', storage);

    expect(readAvatarLabels(storage)).toEqual({
      'avatar-a@secondlife': 'Salty Spicy',
      'avatar-b@grid_test': 'Alt account',
    });
  });

  it('limits labels and removes an alias when it is cleared', () => {
    const storage = memoryStorage();
    saveAvatarLabel('avatar-a@secondlife', 'x'.repeat(MAX_AVATAR_LABEL_LENGTH + 10), storage);
    expect(readAvatarLabels(storage)['avatar-a@secondlife']).toHaveLength(MAX_AVATAR_LABEL_LENGTH);

    saveAvatarLabel('avatar-a@secondlife', '', storage);
    expect(readAvatarLabels(storage)).toEqual({});
  });

  it('ignores malformed saved preferences', () => {
    const storage = memoryStorage();
    storage.setItem('shelfmark:avatar-labels:v1', '{broken');
    expect(readAvatarLabels(storage)).toEqual({});
  });
});
