import { useEffect, useRef, type RefObject } from 'react';

export function useDialogFocus(
  dialogRef: RefObject<HTMLElement | null>,
  initialFocusRef: RefObject<HTMLElement | null>,
  onClose: () => void,
  canClose = true,
) {
  const latest = useRef({ onClose, canClose });
  latest.current = { onClose, canClose };
  useEffect(() => {
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const dialog = dialogRef.current;
    if (!dialog) return;
    (initialFocusRef.current ?? dialog).focus();
    const onKey = (event: KeyboardEvent) => {
      const dialogs = document.querySelectorAll('[role="dialog"]');
      if (dialogs[dialogs.length - 1] !== dialog) return;
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        if (latest.current.canClose) latest.current.onClose();
      }
      if (event.key !== 'Tab') return;
      const focusable = Array.from(dialog.querySelectorAll<HTMLElement>(
        'button:not([disabled]), input:not([disabled]), textarea:not([disabled]), select:not([disabled]), [href], [tabindex]:not([tabindex="-1"])',
      )).filter((element) => element.getClientRects().length > 0);
      if (!focusable.length) { event.preventDefault(); dialog.focus(); return; }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (!dialog.contains(document.activeElement)) { event.preventDefault(); first.focus(); }
      else if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
    };
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('keydown', onKey);
      window.requestAnimationFrame(() => {
        if (dialog.isConnected) return;
        const dialogs = document.querySelectorAll<HTMLElement>('[role="dialog"]');
        const remainingDialog = dialogs[dialogs.length - 1];
        if (previousFocus?.isConnected && (!remainingDialog || remainingDialog.contains(previousFocus))) previousFocus.focus({ preventScroll: true });
        else remainingDialog?.focus({ preventScroll: true });
      });
    };
  }, [dialogRef, initialFocusRef]);
}
