import { describe, expect, it } from 'vitest';
import { candidateReason, matchingPathSegments, normalize, searchItems } from './search';
import type { InventoryItem } from './types';

const items: InventoryItem[] = [
  { id: '1', avatarId: 'a', name: 'Soft wool jacket', path: ['Clothing', 'Winter capsule', 'North Studio'], kind: 'Clothing', createdAt: '2026-09-25', tags: ['outerwear'] },
  { id: '2', avatarId: 'a', name: 'Product information', path: ['Shopping', 'North Studio', 'Jacket'], kind: 'Notecard', createdAt: '2022-01-01', tags: [] },
  { id: '3', avatarId: 'a', name: 'Try-on demo', path: ['Clothing', 'Old demos'], kind: 'Object', createdAt: '2023-01-01', tags: [] },
];

describe('catalogue search', () => {
  it('normalizes accents and punctuation', () => {
    expect(normalize('Crème—Jacket')).toBe('creme jacket');
  });

  it('finds terms remembered from a parent folder', () => {
    expect(searchItems(items, 'winter north')[0]?.item.id).toBe('1');
    expect(searchItems(items, 'jacket north')[0]?.item.id).toBe('2');
  });

  it('allows one-character spelling variation on longer words', () => {
    expect(searchItems(items, 'jackte').map((hit) => hit.item.id)).toContain('1');
  });

  it('keeps candidate reasons explicit and text-based', () => {
    expect(candidateReason(items[2])?.reason).toContain('only a text match');
    expect(candidateReason(items[0])).toBeNull();
  });

  it('uses the clue label computed for a full local snapshot', () => {
    const imported: InventoryItem = { ...items[0], candidateLabel: 'Possible package' };
    expect(candidateReason(imported)).toMatchObject({ label: 'Possible package', confidence: 'Loose' });
  });

  it('finds possible body-fit variants without claiming they are obsolete', () => {
    const legacyFit: InventoryItem = { ...items[0], name: 'Harbor coat — Legacy', kind: 'Clothing' };
    const explicitSize: InventoryItem = { ...items[0], name: 'Wool jacket, size M', kind: 'Object' };
    const note: InventoryItem = { ...items[1], name: 'Maitreya fit notes', kind: 'Notecard' };

    expect(candidateReason(legacyFit)).toMatchObject({ label: 'Possible body-fit variant', confidence: 'Loose' });
    expect(candidateReason(legacyFit)?.reason).toContain('cannot tell whether that fit is old');
    expect(candidateReason(explicitSize)?.label).toBe('Possible body-fit variant');
    expect(candidateReason(note)?.label).toBe('Notecard');
  });

  it('explains same-name groups as an uncertain review clue', () => {
    const imported: InventoryItem = { ...items[0], possibleDuplicate: true };
    expect(candidateReason(imported)).toMatchObject({ label: 'Possible duplicate', confidence: 'Loose' });
    expect(candidateReason(imported)?.reason).toContain('do not prove matching contents');
  });

  it('matches words remembered from the name and the folder trail together', () => {
    const hits = searchItems(items, 'wool winter');
    expect(hits.map((hit) => hit.item.id)).toEqual(['1']);
    expect(hits[0].score).toBeLessThan(1);
  });

  it('ranks single-field matches above matches spread across fields', () => {
    const hits = searchItems(items, 'jacket north');
    expect(hits.map((hit) => hit.item.id)).toEqual(['2', '1']);
  });

  it('does not treat a single unmatched term as a cross-field match', () => {
    expect(searchItems(items, 'velvet')).toEqual([]);
  });

  it('returns every item for punctuation-only queries without crashing', () => {
    expect(searchItems(items, ' — ')).toHaveLength(items.length);
    expect(searchItems([], 'anything')).toEqual([]);
  });

  it('highlights folder segments that match any search term', () => {
    const path = ['Clothing', 'Winter capsule', 'North Studio'];
    expect([...matchingPathSegments(path, 'wool winter')]).toEqual(['Winter capsule']);
    expect([...matchingPathSegments(path, 'NORTH clothing')]).toEqual(['Clothing', 'North Studio']);
    expect(matchingPathSegments(path, '').size).toBe(0);
    expect(matchingPathSegments([], 'winter').size).toBe(0);
  });
});
