import type {
  ButtonHTMLAttributes,
  InputHTMLAttributes,
  ReactNode,
} from "react";
export function Button(props: ButtonHTMLAttributes<HTMLButtonElement>) {
  return <button {...props} className={`button ${props.className ?? ""}`} />;
}
export function Field({
  id,
  label,
  error,
  ...props
}: InputHTMLAttributes<HTMLInputElement> & {
  id: string;
  label: string;
  error?: string;
}) {
  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      <input
        {...props}
        id={id}
        aria-invalid={Boolean(error)}
        aria-describedby={error ? `${id}-error` : undefined}
      />
      {error && (
        <p id={`${id}-error`} role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
export function Status({
  children,
  busy = false,
}: {
  children: ReactNode;
  busy?: boolean;
}) {
  return (
    <p role="status" aria-busy={busy}>
      {children}
    </p>
  );
}
export function CourtyardMark() {
  return (
    <svg
      viewBox="0 0 24 24"
      width="24"
      height="24"
      fill="none"
      stroke="currentColor"
      aria-hidden="true"
    >
      <path d="M12 2C5 2 9 9 12 12C15 9 19 2 12 2ZM22 12C22 5 15 9 12 12C15 15 22 19 22 12ZM12 22C19 22 15 15 12 12C9 15 5 22 12 22ZM2 12C2 19 9 15 12 12C9 9 2 5 2 12Z" />
    </svg>
  );
}
