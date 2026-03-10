import { Component, ErrorInfo, ReactNode } from "react";

export class ErrorBoundary extends Component<{ children: ReactNode }, { hasError: boolean; error: any }> {
  constructor(props: { children: ReactNode }) {
    super(props);
    this.state = { hasError: false, error: null };
  }
  static getDerivedStateFromError(error: any) { return { hasError: true, error }; }
  componentDidCatch(error: any, errorInfo: ErrorInfo) { console.error("ErrorBoundary caught an error", error, errorInfo); }
  render() {
    if (this.state.hasError) {
      return (
        <div className="p-20 bg-red-950 text-primary h-screen overflow-auto font-sans">
          <h1 className="text-2xl font-bold mb-4">Fatal UI Rendering Error</h1>
          <pre className="whitespace-pre-wrap text-[10px] bg-black/30 p-4 rounded mb-4 font-mono">{this.state.error?.toString()}</pre>
          <button onClick={() => window.location.reload()} className="px-4 py-2 bg-red-700 hover:bg-red-600 rounded font-bold transition-colors">Reload Wardian</button>
        </div>
      );
    }
    return this.props.children;
  }
}
