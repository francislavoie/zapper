// Keeps the app usable when part of it throws.
//
// This app draws its own title bar, so an unhandled render error does not
// just lose a pane — it loses the way to close, move and resize the window,
// and leaves a black rectangle with no explanation. The boundary is placed
// so the toolbar is *outside* it: the chrome renders whatever the content
// does, which is the difference between a broken app and an unusable one.
//
// It cannot help when the page itself fails to load, because then there is
// no React. That case is caught in Rust — see `survive_a_failed_interface`.

import { Component, type ErrorInfo, type ReactNode } from "react";

interface Props {
  children: ReactNode;
  /** Named so the message can say which part gave up. */
  what: string;
}

interface State {
  error: Error | null;
}

export class Boundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // The console is the only place with the component stack, and that is
    // the half that says *where*.
    console.error(`${this.props.what} failed to render:`, error, info.componentStack);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="crashed" role="alert">
        <h2>{this.props.what} stopped working</h2>
        <p className="crashed-why" data-selectable>
          {error.message || String(error)}
        </p>
        <p>
          The rest of the app is still running. Reopening usually clears it; if it
          does not, the details are in the developer console.
        </p>
        <button className="ctl" onClick={() => this.setState({ error: null })}>
          Try again
        </button>
      </div>
    );
  }
}
