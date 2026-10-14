// SPDX-License-Identifier: MIT OR Apache-2.0

import { Component, type ErrorInfo, type ReactNode } from "react";
import { Callout } from "./Callout";

interface State {
  error: Error | null;
}

/** Shows render errors instead of a blank window. */
export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(error, info.componentStack);
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <main className="fatal">
        <h1>Something went wrong</h1>
        <Callout
          tone="danger"
          title={this.state.error.message}
          actions={
            <button className="primary small" onClick={() => this.setState({ error: null })}>
              Try again
            </button>
          }
        />
      </main>
    );
  }
}
