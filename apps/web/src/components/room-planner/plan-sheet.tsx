import {
  measurementNanometres,
  type FitResult,
  type MeasurementUnit,
} from "@piwas/room-fit";
import type { PlannerCopy } from "./copy";
import { decimal, zoneInput, type ZoneDraft } from "./draft";
import styles from "./planner.module.css";

function plot(
  zone: ZoneDraft,
  unit: MeasurementUnit,
  scale: number,
  rotate: boolean,
) {
  const input = zoneInput(zone);
  const x = measurementNanometres(input.x, unit);
  const y = measurementNanometres(input.y, unit);
  const width = measurementNanometres(input.width, unit);
  const depth = measurementNanometres(input.depth, unit);
  if (x === null || y === null || !width || !depth) return null;
  const swap = rotate && (zone.rotation === 90 || zone.rotation === 270);
  return {
    x: x * scale,
    y: y * scale,
    width: (swap ? depth : width) * scale,
    height: (swap ? width : depth) * scale,
  };
}

function FitSummary({
  result,
  copy,
}: {
  result: FitResult | null;
  copy: PlannerCopy;
}) {
  if (!result) return <p>{copy.pending}</p>;
  switch (result.status) {
    case "invalid":
      return <p>{copy.invalid}</p>;
    case "fit":
      return <p>{copy.fit}</p>;
    case "uncertain":
      return (
        <p>
          {
            {
              "no-items": copy.noPieces,
              "door-clearances-unknown": copy.unknownDoors,
              "room-tolerance": copy.toleranceTooLarge,
            }[result.reason]
          }
        </p>
      );
    case "conflict":
      return (
        <>
          <p>{copy.conflict}</p>
          <ul className={styles.issues} aria-label={copy.conflict}>
            {result.issues.map((issue) => (
              <li key={JSON.stringify(issue)}>
                {copy.piece} {issue.item + 1}{" "}
                {issue.kind === "outside-room"
                  ? copy.outside
                  : issue.kind === "item-overlap"
                    ? `${copy.overlap} ${copy.piece} ${issue.other + 1}.`
                    : `${copy.blocked} ${copy.door} ${issue.door + 1}.`}
              </li>
            ))}
          </ul>
        </>
      );
  }
}

export function PlanSheet({
  copy,
  unit,
  width,
  depth,
  items,
  doors,
  result,
}: {
  copy: PlannerCopy;
  unit: MeasurementUnit;
  width: string;
  depth: string;
  items: ZoneDraft[];
  doors: ZoneDraft[];
  result: FitResult | null;
}) {
  const w = measurementNanometres(decimal(width), unit);
  const d = measurementNanometres(decimal(depth), unit);
  const scale = w && d ? 380 / Math.max(w, d) : 0;
  return (
    <aside className={styles.sheet} aria-labelledby="plan-drawing">
      <h2 id="plan-drawing">{copy.drawing}</h2>
      <div className={styles.board}>
        {w && d ? (
          <svg
            aria-hidden="true"
            viewBox={`0 0 ${w * scale} ${d * scale}`}
            className={styles.plan}
          >
            {doors.map((door) => {
              const box = plot(door, unit, scale, false);
              return (
                box && (
                  <rect key={door.id} {...box} className={styles.clearance} />
                )
              );
            })}
            {items.map((item, index) => {
              const box = plot(item, unit, scale, true);
              return (
                box && (
                  <g key={item.id}>
                    <rect {...box} className={styles.piece} />
                    {box.width > 18 && box.height > 18 && (
                      <text x={box.x + 6} y={box.y + 14}>
                        {index + 1}
                      </text>
                    )}
                  </g>
                )
              );
            })}
          </svg>
        ) : (
          <p>{copy.emptyDrawing}</p>
        )}
      </div>
      <div role="status" className={styles.result} data-state={result?.status}>
        <FitSummary result={result} copy={copy} />
      </div>
      <p className={styles.note}>{copy.limits}</p>
    </aside>
  );
}
