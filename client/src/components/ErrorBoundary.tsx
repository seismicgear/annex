/**
 * Contains a render crash to the panel it happened in.
 *
 * React unmounts the entire tree when a render throws and nothing catches it,
 * so one bad value anywhere blanked the whole application. The UI audit hit
 * this three separate ways, all from the same shape of mistake — a server
 * response that did not match what the component assumed:
 *
 *   - an events payload without its `events` array killed the Events tab on
 *     `events.length`
 *   - an agent record missing `reputation_score` blanked the app to an empty
 *     black screen on `.toFixed(2)`
 *
 * The individual dereferences are worth hardening (and have been), but no
 * amount of hardening makes "any unexpected value anywhere destroys the app"
 * an acceptable failure mode. A boundary turns it into a dead panel next to a
 * working app, with a way back.
 *
 * Deliberately a class component: `componentDidCatch` / `getDerivedStateFrom-
 * Error` have no hooks equivalent.
 */

import { Component, type ErrorInfo, type ReactNode } from 'react';

interface Props {
  children: ReactNode;
  /** Names the area in the message, e.g. "Events". */
  label?: string;
}

interface State {
  error: Error | null;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // The stack is the only record of what actually happened, and it is worth
    // keeping even though the UI shows a friendly message.
    console.error('[ui] render error contained by boundary', error, info.componentStack);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;

    const where = this.props.label ? ` in ${this.props.label}` : '';
    return (
      <div className="error-boundary" role="alert">
        <p className="error-boundary-title">Something went wrong{where}.</p>
        <p className="error-boundary-hint">
          The rest of the app is still usable. If this keeps happening, reloading
          usually clears it.
        </p>
        <details className="error-details">
          <summary>Details</summary>
          <pre>{error.message}</pre>
        </details>
        <button className="primary-btn" onClick={() => this.setState({ error: null })}>
          Try again
        </button>
      </div>
    );
  }
}
