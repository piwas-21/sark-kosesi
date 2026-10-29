export function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
export function text(value: unknown, maximum: number): value is string {
  return (
    typeof value === "string" &&
    value.trim().length > 0 &&
    Array.from(value).length <= maximum &&
    !value.includes("\0")
  );
}
export function integer(
  value: unknown,
  minimum: number,
  maximum: number,
): value is number {
  return (
    typeof value === "number" &&
    Number.isSafeInteger(value) &&
    value >= minimum &&
    value <= maximum
  );
}
export function merchantCode(value: unknown): value is string {
  return (
    typeof value === "string" &&
    /^[A-Z0-9](?:[A-Z0-9-]{0,63}[A-Z0-9])?$/.test(value) &&
    value !== "DEMO" &&
    !value.startsWith("DEMO-")
  );
}
export function slug(value: unknown): value is string {
  return (
    typeof value === "string" &&
    value.length <= 100 &&
    /^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(value)
  );
}
export function keys(value: Record<string, unknown>, names: string[]): boolean {
  return (
    Object.keys(value).length === names.length &&
    names.every((name) => Object.hasOwn(value, name))
  );
}
export function measurements(value: unknown): boolean {
  return (
    record(value) &&
    keys(value, ["width_mm", "depth_mm", "height_mm", "tolerance_mm"]) &&
    integer(value.width_mm, 1, 10000) &&
    integer(value.depth_mm, 1, 10000) &&
    integer(value.height_mm, 1, 10000) &&
    integer(value.tolerance_mm, 0, 500) &&
    value.tolerance_mm <
      Math.min(value.width_mm, value.depth_mm, value.height_mm)
  );
}
export function decimal(minor: number): string {
  const amount = BigInt(minor);
  return `${amount / 100n}.${String(amount % 100n).padStart(2, "0")}`;
}
