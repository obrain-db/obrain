import { Component, type ReactNode } from "react";

interface P { children: ReactNode; label?: string }
interface S { err: string | null }

/** Attrape toute erreur de rendu canvas et l'affiche (fini les écrans vides). */
export default class CanvasBoundary extends Component<P, S> {
  state: S = { err: null };
  static getDerivedStateFromError(e: unknown): S {
    return { err: e instanceof Error ? `${e.name}: ${e.message}` : String(e) };
  }
  render() {
    if (this.state.err) {
      return <div className="boundary-err">
        <b>{this.props.label || "Vue"} — erreur</b>
        <pre>{this.state.err}</pre>
        <button className="btn" onClick={() => this.setState({ err: null })}>Réessayer</button>
      </div>;
    }
    return this.props.children;
  }
}
