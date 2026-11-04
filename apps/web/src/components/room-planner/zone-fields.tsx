import { Field } from "@piwas/design";
import type { MeasurementUnit } from "@piwas/room-fit";
import type { PlannerCopy } from "./copy";
import {
  badMeasurement,
  coordinates,
  type Coordinate,
  type QuarterTurn,
  type ZoneDraft,
} from "./draft";
import styles from "./planner.module.css";

export function MeasurementField({
  id,
  label,
  value,
  unit,
  positive,
  error,
  onChange,
}: {
  id: string;
  label: string;
  value: string;
  unit: MeasurementUnit;
  positive: boolean;
  error: string | null;
  onChange: (value: string) => void;
}) {
  return (
    <Field
      id={id}
      label={`${label} (${unit})`}
      value={value}
      inputMode="decimal"
      dir="ltr"
      maxLength={16}
      required
      onChange={(event) => onChange(event.target.value)}
      {...(error && badMeasurement(value, unit, positive) ? { error } : {})}
    />
  );
}

export function ZoneFields({
  copy,
  label,
  prefix,
  unit,
  zone,
  rotate,
  showErrors,
  onCoordinate,
  onRotation,
  onRemove,
}: {
  copy: PlannerCopy;
  label: string;
  prefix: string;
  unit: MeasurementUnit;
  zone: ZoneDraft;
  rotate: boolean;
  showErrors: boolean;
  onCoordinate: (key: Coordinate, value: string) => void;
  onRotation?: (angle: QuarterTurn) => void;
  onRemove?: () => void;
}) {
  return (
    <fieldset className={styles.zone}>
      <legend>{label}</legend>
      <div className={styles.fields}>
        {coordinates.map((key) => (
          <MeasurementField
            key={key}
            id={`${prefix}-${zone.id}-${key}`}
            label={copy[key]}
            value={zone[key]}
            unit={unit}
            positive={key === "width" || key === "depth"}
            error={showErrors ? copy.invalid : null}
            onChange={(value) => onCoordinate(key, value)}
          />
        ))}
        {rotate && (
          <label className={styles.control}>
            {copy.rotation}
            <select
              value={zone.rotation}
              onChange={(event) => {
                const angle = Number(event.target.value);
                if (
                  angle === 0 ||
                  angle === 90 ||
                  angle === 180 ||
                  angle === 270
                )
                  onRotation?.(angle);
              }}
            >
              {[0, 90, 180, 270].map((angle) => (
                <option key={angle} value={angle}>
                  {angle}°
                </option>
              ))}
            </select>
          </label>
        )}
      </div>
      {onRemove && (
        <button
          type="button"
          className={styles.textAction}
          aria-label={`${copy.remove} ${label}`}
          onClick={onRemove}
        >
          {copy.remove}
        </button>
      )}
    </fieldset>
  );
}
