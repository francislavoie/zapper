// The red dot on a programme that will be, or is being, recorded. Solid for
// scheduled; pulsing while the recording service is capturing it.

import type { RecordingDto } from "../types";

export function RecDot({ recording }: { recording: Pick<RecordingDto, "state"> }) {
  const live = recording.state === "recording";
  return (
    <span
      className={`rec-dot ${live ? "live" : ""}`}
      role="img"
      aria-label={live ? "Recording" : "Will be recorded"}
    />
  );
}
