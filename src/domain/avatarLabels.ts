const STORAGE_KEY = 'shelfmark:avatar-labels:v1';
export const MAX_AVATAR_LABEL_LENGTH = 48;

type LabelStorage = Pick<Storage, 'getItem' | 'setItem'>;

function storageOrDefault(storage?: LabelStorage): LabelStorage | null {
  if (storage) return storage;
  try {
    return typeof localStorage === 'undefined' ? null : localStorage;
  } catch {
    return null;
  }
}

export function readAvatarLabels(storage?: LabelStorage): Record<string, string> {
  const target = storageOrDefault(storage);
  if (!target) return {};
  try {
    const parsed: unknown = JSON.parse(target.getItem(STORAGE_KEY) ?? '{}');
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return {};
    return Object.fromEntries(Object.entries(parsed)
      .filter(([avatarId, label]) => avatarId.length > 0 && typeof label === 'string' && label.trim().length > 0)
      .map(([avatarId, label]) => [avatarId, (label as string).trim().slice(0, MAX_AVATAR_LABEL_LENGTH)]));
  } catch {
    return {};
  }
}

export function saveAvatarLabel(avatarId: string, value: string, storage?: LabelStorage): Record<string, string> {
  const labels = readAvatarLabels(storage);
  const target = storageOrDefault(storage);
  if (!target || !avatarId.trim()) return labels;

  const label = value.trim().slice(0, MAX_AVATAR_LABEL_LENGTH);
  if (label) labels[avatarId] = label;
  else delete labels[avatarId];

  try {
    target.setItem(STORAGE_KEY, JSON.stringify(labels));
  } catch {
    // An alias is a convenience; an unavailable local preference store must not block import.
  }
  return labels;
}

