// Browser-owned adaptation of Difftastic's display/context.rs line alignment.
const flip = (rows) => rows.map(([left, right]) => [right, left]);
const lines = (text) => text === "" ? [] : text.replace(/\n$/, "").split("\n");
const allLines = (positions) => [...new Set(positions.map((p) => p.pos.line))].sort((a, b) => a - b);
const match = (p) => p.kind.UnchangedToken ?? p.kind.UnchangedPartOfNovelItem;
const novel = (p) => "Novel" in p.kind || "NovelWord" in p.kind;

function mergeOpposite(rows, opposite) {
  const result = [];
  let index = 0;
  for (const [left, right] of rows) {
    while (right !== null && index < opposite.length && opposite[index] <= right) {
      if (opposite[index] < right) result.push([null, opposite[index]]);
      index++;
    }
    result.push([left, right]);
  }
  for (; index < opposite.length; index++) result.push([null, opposite[index]]);
  return result;
}

function pairedRange(left, right, leftEnd, rightEnd) {
  const result = [];
  while (left < leftEnd || right < rightEnd) {
    result.push([left < leftEnd ? left++ : null, right < rightEnd ? right++ : null]);
  }
  return result;
}

function alignedRows(domain, lhs, rhs) {
  if (domain.lhs_src.Text === domain.rhs_src.Text) return lhs.map((_, i) => [i, i]);
  let rows = [], highestLeft = -1, highestRight = -1;
  for (const position of domain.lhs_positions) {
    const right = match(position)?.opposite_pos.find((span) => span.line > highestRight)?.line;
    if (right === undefined || position.pos.line <= highestLeft) continue;
    highestLeft = position.pos.line;
    highestRight = right;
    rows.push([highestLeft, highestRight]);
  }
  rows = mergeOpposite(rows, allLines(domain.rhs_positions));
  rows = flip(mergeOpposite(flip(rows), allLines(domain.lhs_positions)));

  const firstLeft = rows.find(([l]) => l !== null)?.[0];
  const firstRight = rows.find(([, r]) => r !== null)?.[1];
  const lastLeft = rows.findLast(([l]) => l !== null)?.[0];
  const lastRight = rows.findLast(([, r]) => r !== null)?.[1];
  if (firstLeft !== undefined && firstRight !== undefined) {
    rows = [...pairedRange(0, 0, firstLeft, firstRight), ...rows];
  }
  if (lastLeft !== undefined && lastRight !== undefined) {
    rows.push(...pairedRange(lastLeft + 1, lastRight + 1, lhs.length, rhs.length));
  }

  // Blank lines just before a matched row align from the bottom of the gap.
  const blanks = [];
  let prevLeft = null, prevRight = null;
  for (const [left, right] of rows) {
    if (left !== null && right !== null) {
      const before = [];
      const first = prevLeft === null && prevRight === null;
      const later = prevLeft !== null && prevRight !== null;
      if (first || later) {
        for (let l = left - 1, r = right - 1;
          l >= 0 && r >= 0 && (first || (l > prevLeft && r > prevRight)); l--, r--) {
          if (lhs[l] !== "" || rhs[r] !== "") break;
          before.push([l, r]);
        }
      }
      blanks.push(...before.reverse());
    }
    blanks.push([left, right]);
    if (left !== null) prevLeft = left;
    if (right !== null) prevRight = right;
  }

  const contiguous = [];
  prevLeft = prevRight = null;
  for (const [left, right] of blanks) {
    if (left !== null && prevLeft !== null) {
      for (let l = prevLeft + 1; l < left; l++) contiguous.push([l, null]);
    }
    if (right !== null && prevRight !== null) {
      for (let r = prevRight + 1; r < right; r++) contiguous.push([null, r]);
    }
    contiguous.push([left, right]);
    if (left !== null) prevLeft = left;
    if (right !== null) prevRight = right;
  }

  const compact = [], pending = [];
  for (const [left, right] of contiguous) {
    if (left !== null && right === null && pending[0]?.[0] === null) {
      compact.push([left, pending.shift()[1]]);
    } else if (right !== null && left === null && pending[0]?.[1] === null) {
      compact.push([pending.shift()[0], right]);
    } else if (left === null || right === null) {
      pending.push([left, right]);
    } else {
      compact.push(...pending.splice(0), [left, right]);
    }
  }
  compact.push(...pending);
  const seenLeft = new Set(), seenRight = new Set();
  return compact.map(([left, right]) => {
    if (left !== null && (left >= lhs.length || seenLeft.has(left))) left = null;
    if (right !== null && (right >= rhs.length || seenRight.has(right))) right = null;
    if (left !== null) seenLeft.add(left);
    if (right !== null) seenRight.add(right);
    return [left, right];
  }).filter(([l, r]) => l !== null || r !== null);
}

export function deriveLayout(domain) {
  const lhs = lines(domain.lhs_src.Text), rhs = lines(domain.rhs_src.Text);
  const baseline = [new Set(), new Set()];
  for (const hunk of domain.hunks) {
    for (const [left, right] of hunk.lines) {
      if (left !== null) baseline[0].add(left);
      if (right !== null) baseline[1].add(right);
    }
  }
  const novelLeft = new Set(domain.lhs_positions.filter(novel).map((p) => p.pos.line));
  const novelRight = new Set(domain.rhs_positions.filter(novel).map((p) => p.pos.line));
  const reindented = new Map();
  for (const p of domain.lhs_positions) {
    const unchanged = p.kind.UnchangedToken;
    if (!unchanged || novelLeft.has(p.pos.line)) continue;
    for (const opposite of unchanged.opposite_pos) {
      const l = p.pos.line, r = opposite.line;
      if (novelRight.has(r) || lhs[l] === undefined || rhs[r] === undefined) continue;
      if (lhs[l] !== rhs[r] && lhs[l].trimStart() === rhs[r].trimStart()) {
        reindented.set(`${l}:${r}`, [l, r]);
      }
    }
  }
  return { rows: alignedRows(domain, lhs, rhs), baseline: baseline.map((s) => [...s]), reindented: [...reindented.values()] };
}
