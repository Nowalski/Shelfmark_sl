const DATABASE_NAME = 'shelfmark-local-media';
const DATABASE_VERSION = 1;
const MEDIA_STORE = 'images';

type MediaRecord = { key: string; image: Blob };
export type LocalMediaRecord = { key: string; image: Blob };

function openMediaDatabase(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    if (!('indexedDB' in window)) {
      reject(new Error('Local image storage is unavailable in this browser profile.'));
      return;
    }
    const request = indexedDB.open(DATABASE_NAME, DATABASE_VERSION);
    let abandoned = false;
    request.onupgradeneeded = () => {
      const database = request.result;
      if (!database.objectStoreNames.contains(MEDIA_STORE)) {
        database.createObjectStore(MEDIA_STORE, { keyPath: 'key' });
      }
    };
    request.onsuccess = () => {
      const database = request.result;
      if (abandoned) { database.close(); return; }
      database.onversionchange = () => database.close();
      resolve(database);
    };
    request.onerror = () => reject(request.error ?? new Error('Could not open local image storage.'));
    request.onblocked = () => {
      abandoned = true;
      reject(new Error('Close another Shelfmark window to update local image storage.'));
    };
  });
}

export async function saveLocalImage(key: string, image: Blob): Promise<void> {
  const database = await openMediaDatabase();
  try {
    await new Promise<void>((resolve, reject) => {
      const transaction = database.transaction(MEDIA_STORE, 'readwrite');
      transaction.objectStore(MEDIA_STORE).put({ key, image } satisfies MediaRecord);
      transaction.oncomplete = () => resolve();
      transaction.onerror = () => reject(transaction.error ?? new Error('Could not save this image locally.'));
      transaction.onabort = () => reject(transaction.error ?? new Error('Could not save this image locally.'));
    });
  } finally {
    database.close();
  }
}

export async function readLocalImage(key: string): Promise<Blob | null> {
  return (await readLocalImages([key])).get(key) ?? null;
}

export async function readLocalImages(keys: string[]): Promise<Map<string, Blob>> {
  if (!keys.length) return new Map();
  const database = await openMediaDatabase();
  try {
    return await new Promise<Map<string, Blob>>((resolve, reject) => {
      const images = new Map<string, Blob>();
      const transaction = database.transaction(MEDIA_STORE, 'readonly');
      const store = transaction.objectStore(MEDIA_STORE);
      for (const key of new Set(keys)) {
        const request = store.get(key);
        request.onsuccess = () => {
          const image = (request.result as MediaRecord | undefined)?.image;
          if (image) images.set(key, image);
        };
      }
      transaction.oncomplete = () => resolve(images);
      transaction.onerror = () => reject(transaction.error ?? new Error('Could not read local images.'));
      transaction.onabort = () => reject(transaction.error ?? new Error('Reading local images was interrupted.'));
    });
  } finally {
    database.close();
  }
}

export async function removeLocalImage(key: string): Promise<void> {
  const database = await openMediaDatabase();
  try {
    await new Promise<void>((resolve, reject) => {
      const transaction = database.transaction(MEDIA_STORE, 'readwrite');
      transaction.objectStore(MEDIA_STORE).delete(key);
      transaction.oncomplete = () => resolve();
      transaction.onerror = () => reject(transaction.error ?? new Error('Could not remove this local image.'));
      transaction.onabort = () => reject(transaction.error ?? new Error('Could not remove this local image.'));
    });
  } finally {
    database.close();
  }
}

export async function listLocalImages(): Promise<LocalMediaRecord[]> {
  const database = await openMediaDatabase();
  try {
    return await new Promise<LocalMediaRecord[]>((resolve, reject) => {
      const transaction = database.transaction(MEDIA_STORE, 'readonly');
      const request = transaction.objectStore(MEDIA_STORE).getAll();
      let images: LocalMediaRecord[] = [];
      request.onsuccess = () => { images = (request.result as MediaRecord[]).sort((a, b) => a.key.localeCompare(b.key)); };
      transaction.oncomplete = () => resolve(images);
      transaction.onerror = () => reject(transaction.error ?? new Error('Could not read local Shelfmark photos.'));
      transaction.onabort = () => reject(transaction.error ?? new Error('Reading local Shelfmark photos was interrupted.'));
    });
  } finally {
    database.close();
  }
}

export async function replaceLocalImages(images: LocalMediaRecord[]): Promise<void> {
  const database = await openMediaDatabase();
  try {
    await new Promise<void>((resolve, reject) => {
      const transaction = database.transaction(MEDIA_STORE, 'readwrite');
      const store = transaction.objectStore(MEDIA_STORE);
      store.clear();
      for (const image of images) store.put(image satisfies MediaRecord);
      transaction.oncomplete = () => resolve();
      transaction.onerror = () => reject(transaction.error ?? new Error('Could not restore local Shelfmark photos.'));
      transaction.onabort = () => reject(transaction.error ?? new Error('Could not restore local Shelfmark photos.'));
    });
  } finally {
    database.close();
  }
}

export const itemImageKey = (avatarId: string, itemId: string) => `${avatarId}:item:${itemId}`;
export const collectionImageKey = (avatarId: string, collectionId: string) => `${avatarId}:collection:${collectionId}`;
