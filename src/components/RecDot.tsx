// The dot on a programme that will be, or is being, recorded. Red: solid
// for scheduled, pulsing while the recording service is capturing it. Grey
// once it has stopped and is being saved into its file.

import type { RecordingDto } from "../types";

export function RecDot({ recording }: { recording: Pick<RecordingDto, "state"> }) {
  const live = recording.state === "recording";
  const saving = recording.state === "finishing";
  return (
    <span
      className={`rec-dot ${live ? "live" : ""} ${saving ? "saving" : ""}`}
      role="img"
      aria-label={live ? "Recording" : saving ? "Saving the recording" : "Will be recorded"}
    />
  );
}
