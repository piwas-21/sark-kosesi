// One capture/comparison environment; viewport changes alone do not reset every
// engine's screen/DPR defaults consistently.
export function visualContext(viewport) {
  return {
    viewport,
    screen: viewport,
    deviceScaleFactor: 1,
    reducedMotion: "reduce",
  };
}
