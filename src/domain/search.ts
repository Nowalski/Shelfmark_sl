import type { InventoryItem, ItemKind, SearchHit } from './types';

export type SearchOptions = {
  kind?: ItemKind | 'Any type';
  tags?: Record<string, string[]>;
  includeDescription?: boolean;
};

export function normalize(value: string): string {
  return value
    .normalize('NFKD')
    .replace(/[\u0300-\u036f]/g, '')
    .toLocaleLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, ' ')
    .trim();
}

function distanceWithinOne(a: string, b: string): boolean {
  if (Math.abs(a.length - b.length) > 1 || a.length < 4 || b.length < 4) return false;
  const left = Array.from(a);
  const right = Array.from(b);
  if (left.length === right.length) {
    const mismatches: number[] = [];
    for (let index = 0; index < left.length; index += 1) {
      if (left[index] !== right[index]) mismatches.push(index);
      if (mismatches.length > 2) return false;
    }
    if (mismatches.length <= 1) return true;
    const [first, second] = mismatches;
    return second === first + 1 && left[first] === right[second] && left[second] === right[first];
  }
  const longer = left.length > right.length ? left : right;
  const shorter = left.length > right.length ? right : left;
  let i = 0;
  let j = 0;
  let skipped = false;
  while (i < longer.length && j < shorter.length) {
    if (longer[i] === shorter[j]) { i += 1; j += 1; }
    else if (skipped) return false;
    else { skipped = true; i += 1; }
  }
  return true;
}

function fuzzyContains(haystack: string, terms: string[]): boolean {
  const words = haystack.split(' ');
  return terms.every((term) => words.some((word) => word.startsWith(term) || distanceWithinOne(word, term)));
}

export function searchItems(
  items: InventoryItem[],
  query: string,
  options: SearchOptions = {},
): SearchHit[] {
  const terms = normalize(query).split(' ').filter(Boolean);
  return items.flatMap((item) => {
    if (options.kind && options.kind !== 'Any type' && item.kind !== options.kind) return [];
    const tags = options.tags?.[item.id] ?? item.tags;
    const fields: [SearchHit['match'], string, number][] = [
      ['Name', normalize(item.name), 3],
      ['Folder path', normalize(item.path.join(' ')), 2],
      ['Tag', normalize(tags.join(' ')), 2],
    ];
    if (options.includeDescription !== false && item.description) {
      fields.push(['Description', normalize(item.description), 1]);
    }
    if (!terms.length) return [{ item, match: 'Name' as const, score: 0 }];
    const hit = fields.find(([, text]) => fuzzyContains(text, terms));
    return hit ? [{ item, match: hit[0], score: hit[2] }] : [];
  }).sort((a, b) => b.score - a.score || a.item.name.localeCompare(b.item.name));
}

export function candidateReason(item: InventoryItem): { label: string; reason: string; confidence: 'Loose' | 'Close' } | null {
  if (item.possibleDuplicate) {
    return { label: 'Possible duplicate', reason: 'The same normalized name and broad inventory type appear more than once in this avatar snapshot. Matching names do not prove matching contents; compare the folder, creator, and item details before deciding.', confidence: 'Loose' };
  }
  if (item.candidateLabel === 'Demo-like name') {
    return { label: 'Demo-like name', reason: 'A name or folder contains “demo”. Review the item itself; this is only a text match.', confidence: 'Loose' };
  }
  if (item.candidateLabel === 'Possible package') {
    return { label: 'Possible package', reason: 'A name or folder suggests packaging. It may still be useful or unopened.', confidence: 'Loose' };
  }
  if (item.candidateLabel === 'Possible body-fit variant') {
    return { label: 'Possible body-fit variant', reason: 'A name or folder mentions a common avatar-body fit or clothing size. This snapshot cannot tell whether that fit is old or one you still wear; review it before making any inventory change.', confidence: 'Loose' };
  }
  if (item.candidateLabel === 'Notecard') {
    return { label: 'Notecard', reason: 'This item is a notecard. Its content and purpose are not inspected.', confidence: 'Close' };
  }
  if (item.candidateLabel === 'Landmark') {
    return { label: 'Landmark', reason: 'This item is a landmark. Whether its destination still exists is unknown.', confidence: 'Close' };
  }
  const text = normalize(`${item.name} ${item.path.join(' ')}`);
  if (/\b(demo|try on|tryon)\b/.test(text)) {
    return { label: 'Demo-like name', reason: 'A name or folder contains “demo”. Review the item itself; this is only a text match.', confidence: 'Loose' };
  }
  if (/\b(unpack|box|package|delivery)\b/.test(text) && item.kind === 'Object') {
    return { label: 'Possible package', reason: 'A name or folder suggests packaging. It may still be useful or unopened.', confidence: 'Loose' };
  }
  if (hasBodyFitVariantClue(text, item.kind)) {
    return { label: 'Possible body-fit variant', reason: 'A name or folder mentions a common avatar-body fit or clothing size. This snapshot cannot tell whether that fit is old or one you still wear; review it before making any inventory change.', confidence: 'Loose' };
  }
  if (item.kind === 'Notecard') {
    return { label: 'Notecard', reason: 'This item is a notecard. Its content and purpose are not inspected.', confidence: 'Close' };
  }
  if (item.kind === 'Landmark') {
    return { label: 'Landmark', reason: 'This item is a landmark. Whether its destination still exists is unknown.', confidence: 'Close' };
  }
  return null;
}

const BODY_FIT_TERMS = new Set([
  'belleza', 'ebody', 'freya', 'genx', 'gianni', 'hourglass', 'isis', 'jake', 'kario', 'kupra',
  'lara', 'legacy', 'maitreya', 'physique', 'reborn', 'signature', 'slink', 'tmp', 'venus',
]);
const EXTENDED_CLOTHING_SIZES = new Set(['xxs', 'xs', 'xl', 'xxl']);
const CLOTHING_SIZE_MARKERS = new Set(['xxs', 'xs', 's', 'm', 'l', 'xl', 'xxl']);

function hasBodyFitVariantClue(normalizedNameAndPath: string, kind: ItemKind): boolean {
  if (kind !== 'Clothing' && kind !== 'Object') return false;
  const words = normalizedNameAndPath.split(' ');
  if (words.some((word) => BODY_FIT_TERMS.has(word))) return true;
  if (words.some((word) => word === 'size' || word === 'sizes')) return true;
  if (words.some((word) => EXTENDED_CLOTHING_SIZES.has(word))) return true;
  return words.some((word) => word === 'body' || word === 'fit' || word === 'fitted')
    && words.some((word) => CLOTHING_SIZE_MARKERS.has(word));
}
