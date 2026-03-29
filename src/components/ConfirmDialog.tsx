import React, { createContext, useContext, useState, useCallback } from "react";

type ConfirmFn = (message: string) => Promise<boolean>;

const ConfirmContext = createContext<ConfirmFn>(async () => false);

export const useConfirm = () => useContext(ConfirmContext);

interface ConfirmState {
  open: boolean;
  message: string;
  resolve: ((value: boolean) => void) | null;
}

export const ConfirmProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const [state, setState] = useState<ConfirmState>({ open: false, message: "", resolve: null });

  const confirm = useCallback((message: string): Promise<boolean> => {
    return new Promise(resolve => {
      setState({ open: true, message, resolve });
    });
  }, []);

  const settle = (value: boolean) => {
    setState(prev => ({ ...prev, open: false }));
    state.resolve?.(value);
  };

  return (
    <ConfirmContext.Provider value={confirm}>
      {children}
      {state.open && (
        <div
          id="confirm-dialog-overlay"
          className="fixed inset-0 z-[11000] flex items-center justify-center"
          style={{ backgroundColor: "var(--color-wardian-overlay)" }}
          onClick={() => settle(false)}
        >
          <div
            id="confirm-dialog-panel"
            className="relative rounded-lg p-6 max-w-sm w-full mx-4 shadow-2xl"
            style={{
              background: "var(--color-wardian-sidebar-primary)",
              border: "1px solid var(--color-wardian-border-heavy)",
            }}
            onClick={e => e.stopPropagation()}
          >
            <p className="text-sm text-primary mb-6 leading-relaxed">{state.message}</p>
            <div className="flex gap-2 justify-end">
              <button
                id="confirm-dialog-cancel"
                onClick={() => settle(false)}
                className="px-4 py-2 text-xs font-bold rounded transition-colors text-muted hover:text-primary"
                style={{
                  background: "var(--color-wardian-card-bg-muted)",
                  border: "1px solid var(--color-wardian-border)",
                }}
              >
                Cancel
              </button>
              <button
                id="confirm-dialog-confirm"
                onClick={() => settle(true)}
                className="px-4 py-2 text-xs font-bold rounded transition-colors"
                style={{
                  background: "var(--color-wardian-error)",
                  color: "white",
                }}
              >
                Confirm
              </button>
            </div>
          </div>
        </div>
      )}
    </ConfirmContext.Provider>
  );
};

