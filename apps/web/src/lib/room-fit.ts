/** Local, approximate rectangular planning only; inputs are never catalogue facts. */
export type MeasurementUnit = "mm" | "cm" | "in";

type Rectangle = { x: number; y: number; width: number; depth: number };
type Item = Rectangle & { rotation: 0 | 90 | 180 | 270 };
type FitIssue =
  | { kind: "outside-room"; item: number }
  | { kind: "item-overlap"; item: number; other: number }
  | { kind: "door-clearance"; item: number; door: number };

export type FitResult =
  | { status: "invalid" }
  | {
      status: "uncertain";
      reason: "no-items" | "door-clearances-unknown" | "room-tolerance";
    }
  | { status: "conflict"; issues: FitIssue[] }
  | { status: "fit" };

const NANOMETRES_PER_UNIT = { mm: 1_000_000, cm: 10_000_000, in: 25_400_000 };
const MAX_LENGTH = 100_000_000_000; // 100 m in nanometres
const MAX_ITEMS = 50;
const MAX_DOORS = 20;

/** At most three decimal places; conversion is exact, including thousandths of an inch. */
export function measurementNanometres(
  value: unknown,
  unit: unknown,
): number | null {
  if (
    typeof value !== "string" ||
    !/^(?:0|[1-9]\d{0,5})(?:\.\d{1,3})?$/.test(value) ||
    (unit !== "mm" && unit !== "cm" && unit !== "in")
  ) {
    return null;
  }
  const [whole, fraction = ""] = value.split(".");
  const thousandths = Number(whole) * 1000 + Number(fraction.padEnd(3, "0"));
  const result = (thousandths * NANOMETRES_PER_UNIT[unit]) / 1000;
  return Number.isSafeInteger(result) && result <= MAX_LENGTH ? result : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function rectangle(value: unknown, unit: unknown): Rectangle | null {
  if (!isRecord(value)) return null;
  const x = measurementNanometres(value.x, unit);
  const y = measurementNanometres(value.y, unit);
  const width = measurementNanometres(value.width, unit);
  const depth = measurementNanometres(value.depth, unit);
  if (
    x === null ||
    y === null ||
    width === null ||
    depth === null ||
    width === 0 ||
    depth === 0
  ) {
    return null;
  }
  return { x, y, width, depth };
}

function item(value: unknown, unit: unknown): Item | null {
  if (!isRecord(value)) return null;
  const rect = rectangle(value, unit);
  const rotation = value.rotation;
  if (
    !rect ||
    (rotation !== 0 && rotation !== 90 && rotation !== 180 && rotation !== 270)
  ) {
    return null;
  }
  return { ...rect, rotation };
}

function inflate(rect: Rectangle, tolerance: number): Rectangle {
  return {
    x: rect.x - tolerance,
    y: rect.y - tolerance,
    width: rect.width + 2 * tolerance,
    depth: rect.depth + 2 * tolerance,
  };
}

function overlaps(a: Rectangle, b: Rectangle): boolean {
  return (
    a.x < b.x + b.width &&
    b.x < a.x + a.width &&
    a.y < b.y + b.depth &&
    b.y < a.y + a.depth
  );
}

/**
 * All lengths share one unit; coordinates measure from the room's top-left.
 * The item's top-left remains fixed when rotating. No physical fit is guaranteed.
 */
export function assessRoomFit(input: unknown): FitResult {
  if (
    !isRecord(input) ||
    !isRecord(input.room) ||
    !Array.isArray(input.items)
  ) {
    return { status: "invalid" };
  }
  const unit = input.unit;
  const width = measurementNanometres(input.room.width, unit);
  const depth = measurementNanometres(input.room.depth, unit);
  const tolerance = measurementNanometres(input.tolerance, unit);
  if (
    width === null ||
    depth === null ||
    tolerance === null ||
    width === 0 ||
    depth === 0 ||
    input.items.length > MAX_ITEMS ||
    (input.doorClearances !== null &&
      (!Array.isArray(input.doorClearances) ||
        input.doorClearances.length > MAX_DOORS))
  ) {
    return { status: "invalid" };
  }
  const items: Item[] = [];
  for (const value of input.items) {
    const parsed = item(value, unit);
    if (!parsed) return { status: "invalid" };
    items.push(parsed);
  }
  const doors: Rectangle[] = [];
  if (Array.isArray(input.doorClearances)) {
    for (const value of input.doorClearances) {
      const parsed = rectangle(value, unit);
      if (!parsed) return { status: "invalid" };
      doors.push(inflate(parsed, tolerance));
    }
  }
  if (2 * tolerance >= width || 2 * tolerance >= depth) {
    return { status: "uncertain", reason: "room-tolerance" };
  }
  if (items.length === 0) return { status: "uncertain", reason: "no-items" };
  const expanded = items.map(({ rotation, ...rect }) =>
    inflate(
      rotation === 90 || rotation === 270
        ? { ...rect, width: rect.depth, depth: rect.width }
        : rect,
      tolerance,
    ),
  );
  const issues: FitIssue[] = [];
  expanded.forEach((current, index) => {
    if (
      current.x < tolerance ||
      current.y < tolerance ||
      current.x + current.width > width - tolerance ||
      current.y + current.depth > depth - tolerance
    ) {
      issues.push({ kind: "outside-room", item: index });
    }
    expanded.slice(0, index).forEach((other, previous) => {
      if (overlaps(current, other))
        issues.push({ kind: "item-overlap", item: index, other: previous });
    });
    doors.forEach((door, doorIndex) => {
      if (overlaps(current, door))
        issues.push({ kind: "door-clearance", item: index, door: doorIndex });
    });
  });
  if (issues.length > 0) return { status: "conflict", issues };
  if (input.doorClearances === null)
    return { status: "uncertain", reason: "door-clearances-unknown" };
  return { status: "fit" };
}
