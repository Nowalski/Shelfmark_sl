// Object URLs avoid copying the original file into a base64 string before
// decoding. The resized image is stored locally by mediaStore.
export function resizeImage(file: File): Promise<Blob> {
  if (!['image/jpeg', 'image/png', 'image/webp'].includes(file.type)) return Promise.reject(new Error('Choose a JPEG, PNG or WebP image.'));
  if (file.size > 8 * 1024 * 1024) return Promise.reject(new Error('Choose an image under 8 MB.'));
  return new Promise((resolve, reject) => {
    const image = new Image();
    const sourceUrl = URL.createObjectURL(file);
    let settled = false;
    const timer = window.setTimeout(() => finish(new Error('Preparing this photo took too long. Try a smaller image.')), 30_000);
    const cleanup = () => {
      window.clearTimeout(timer);
      URL.revokeObjectURL(sourceUrl);
      image.onload = null;
      image.onerror = null;
      image.removeAttribute('src');
    };
    function finish(result: Blob | Error) {
      if (settled) return;
      settled = true;
      cleanup();
      if (result instanceof Blob) resolve(result);
      else reject(result);
    }
    image.onerror = () => finish(new Error('That image format could not be previewed.'));
    image.onload = () => {
      try {
        const width = image.naturalWidth;
        const height = image.naturalHeight;
        if (!width || !height || width > 20_000 || height > 20_000 || width * height > 25_000_000) throw new Error('That image is too large to prepare safely.');
        const scale = Math.min(1, 960 / width, 640 / height);
        const canvas = document.createElement('canvas');
        canvas.width = Math.max(1, Math.round(width * scale));
        canvas.height = Math.max(1, Math.round(height * scale));
        const context = canvas.getContext('2d');
        if (!context) throw new Error('Could not prepare that image on this device.');
        // JPEG has no alpha channel. Give transparent outfit references a
        // neutral backdrop rather than silently turning their background black.
        context.fillStyle = '#ffffff';
        context.fillRect(0, 0, canvas.width, canvas.height);
        context.drawImage(image, 0, 0, canvas.width, canvas.height);
        canvas.toBlob((blob) => finish(blob ?? new Error('Could not prepare that image on this device.')), 'image/jpeg', 0.78);
      } catch (error) {
        finish(error instanceof Error ? error : new Error('Could not prepare that image on this device.'));
      }
    };
    try { image.src = sourceUrl; }
    catch { finish(new Error('Could not open that image on this device.')); }
  });
}
