import { Component, type ErrorInfo, type ReactNode } from "react";

interface State {
  error?: Error;
  where?: string;
}

/** A render error shows what broke and a reload button, instead of a blank (black) window. */
export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = {};

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error("op-workflow UI error", error, info.componentStack);
    this.setState({ where: info.componentStack?.trim().split("\n").slice(0, 6).join("\n") });
  }

  render(): ReactNode {
    const { error, where } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="crash" role="alert">
        <h1>Giao diện gặp lỗi</h1>
        <p>Dữ liệu và các bước đang chạy không bị ảnh hưởng. Chụp màn hình này gửi lại để sửa, rồi bấm Tải lại.</p>
        <pre>
          {error.name}: {error.message}
          {where ? `\n\n${where}` : ""}
          {error.stack ? `\n\n${error.stack.split("\n").slice(0, 8).join("\n")}` : ""}
        </pre>
        <button className="btn btn--primary" onClick={() => window.location.reload()}>
          Tải lại
        </button>
      </div>
    );
  }
}
