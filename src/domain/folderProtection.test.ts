import { describe, expect, it } from 'vitest';
import { classifyItemSafety, createFolderProtectionResolver } from './folderProtection';

const ROOT = 'root';
const folders = [
  { id: 'wardrobe', parentId: ROOT },
  { id: 'outerwear', parentId: 'wardrobe' },
  { id: 'coats', parentId: 'outerwear' },
  { id: 'shoes', parentId: 'wardrobe' },
];

describe('folder keep-safe inheritance', () => {
  it('protects the marked folder and every known descendant', () => {
    const protection = createFolderProtectionResolver(folders, ['outerwear'], ROOT);

    expect(protection.isMarked('outerwear')).toBe(true);
    expect(protection.isProtected('outerwear')).toBe(true);
    expect(protection.isMarked('coats')).toBe(false);
    expect(protection.isProtected('coats')).toBe(true);
    expect(protection.isProtected('shoes')).toBe(false);
  });

  it('reports direct child marks and safely ignores missing or cyclic paths', () => {
    const cyclicFolders = [...folders, { id: 'cycle-a', parentId: 'cycle-b' }, { id: 'cycle-b', parentId: 'cycle-a' }];
    const protection = createFolderProtectionResolver(cyclicFolders, ['shoes'], ROOT);

    expect(protection.isMarked('shoes')).toBe(true);
    expect(protection.isProtected('wardrobe')).toBe(false);
    expect(protection.isProtected('missing')).toBe(false);
    expect(protection.isProtected('cycle-a')).toBe(false);
  });

  it('keeps Firestorm special folders protected through their descendants', () => {
    const specialFolders = [
      { id: 'current', parentId: ROOT, name: 'Current Outfit', preferredType: 'current' },
      { id: 'current-child', parentId: 'current', name: 'Attachments', preferredType: '-1' },
      { id: 'market-stock', parentId: ROOT, name: 'Stock', preferredType: 'stock' },
      { id: 'ordinary', parentId: ROOT, name: 'My notes', preferredType: '-1' },
      { id: 'future-system', parentId: ROOT, name: 'Future folder', preferredType: 'new_system_type' },
      { id: 'unknown', parentId: ROOT, name: 'Type missing', preferredType: '' },
    ];
    const protection = createFolderProtectionResolver(specialFolders, [], ROOT);

    expect(protection.getSafety('current').systemFolder?.id).toBe('current');
    expect(protection.getSafety('current-child').systemFolder?.id).toBe('current');
    expect(protection.isProtected('market-stock')).toBe(true);
    expect(protection.isProtected('future-system')).toBe(true);
    expect(protection.isProtected('ordinary')).toBe(false);
    expect(protection.isProtected('unknown')).toBe(false);
  });

  it('marks links, recorded no-copy items, and items in protected parent folders', () => {
    const protection = createFolderProtectionResolver([
      { id: 'trash', parentId: ROOT, name: 'Trash', preferredType: 'trash' },
      { id: 'normal', parentId: ROOT, name: 'Wardrobe', preferredType: '-1' },
    ], ['normal'], ROOT);
    const copyableMask = 0x8000 | 0x4000 | 0x2000;

    expect(classifyItemSafety({ id: 'link', isLink: true }, [], protection).reasons.map((reason) => reason.source))
      .toEqual(['inventory-link']);
    expect(classifyItemSafety({ id: 'no-copy', ownerMask: 0x4000 }, [], protection).reasons.map((reason) => reason.source))
      .toEqual(['no-copy']);
    expect(classifyItemSafety({ id: 'unknown', ownerMask: undefined }, [], protection).isProtected).toBe(false);
    expect(classifyItemSafety({ id: 'copy', ownerMask: copyableMask, parentFolderId: 'normal' }, [], protection).reasons.map((reason) => reason.source))
      .toEqual(['folder-mark']);
    expect(classifyItemSafety({ id: 'trash-item', ownerMask: copyableMask, parentFolderId: 'trash' }, [], protection).reasons.map((reason) => reason.source))
      .toEqual(['system-folder']);
    expect(classifyItemSafety({ id: 'manual', ownerMask: copyableMask }, ['manual'], protection).isItemMarked).toBe(true);
  });
});
