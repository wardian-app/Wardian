import { useRef, useState, useEffect, useCallback, memo } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { Unicode11Addon } from '@xterm/addon-unicode11';
import '@xterm/xterm/css/xterm.css';

const DARK_TERM_THEME = {
  background: '#020402',
  foreground: '#EEF2EE',
  cursor: '#F1D382',
  selectionBackground: '#1E261E'
};

const LIGHT_TERM_THEME = {
  background: '#fcfaf5',
  foreground: '#111827',
  cursor: '#b8860b',
  selectionBackground: '#e5e7eb'
};

const terminalMap = new Map<string, Terminal>();
const fitAddonMap = new Map<string, FitAddon>();

export const AgentTerminal = memo(function AgentTerminal({ 
  sessionId, 
  isMaximized, 
  theme, 
  onTitleChange 
}: { 
  sessionId: string; 
  isMaximized?: boolean; 
  theme: 'dark' | 'light' | 'system'; 
  onTitleChange?: (title: string) => void 
}) {
  const terminalRef = useRef<HTMLDivElement>(null);
  const xtermRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const pollStartedRef = useRef(false);
  const onTitleChangeRef = useRef(onTitleChange);
  const [initError, setInitError] = useState<string | null>(null);

  const [effectiveTheme, setEffectiveTheme] = useState<'dark' | 'light'>(() => {
    if (theme === 'system') return window.matchMedia("(prefers-color-scheme: light)").matches ? 'light' : 'dark';
    return theme;
  });

  useEffect(() => {
    if (theme !== 'system') {
      setEffectiveTheme(theme);
      return;
    }
    const mediaQuery = window.matchMedia("(prefers-color-scheme: light)");
    const handler = () => setEffectiveTheme(mediaQuery.matches ? 'light' : 'dark');
    handler();
    mediaQuery.addEventListener("change", handler);
    return () => mediaQuery.removeEventListener("change", handler);
  }, [theme]);

  const termTheme = effectiveTheme === 'light' ? LIGHT_TERM_THEME : DARK_TERM_THEME;

  useEffect(() => {
    onTitleChangeRef.current = onTitleChange;
  }, [onTitleChange]);

  const performFit = useCallback((forceInvoke = false) => {
    const term = xtermRef.current;
    const fitAddon = fitAddonRef.current;
    if (!term || !fitAddon || !terminalRef.current) return;
    if (terminalRef.current.offsetParent === null) return;

    try {
      fitAddon.fit();
      if (term.cols > 10 && term.rows > 3) {
        if (forceInvoke) {
          invoke("resize_agent_terminal", { sessionId, cols: term.cols, rows: term.rows }).catch(() => {});
        }
      }
    } catch { /* ignore */ }
  }, [sessionId]);

  useEffect(() => {
    if (!sessionId || !terminalRef.current) return;
    
    let isMounted = true;
    let pollActive = true;
    let resizeObserver: ResizeObserver | null = null;
    let term: Terminal | null = null;
    let fitAddon: FitAddon | null = null;

    try {
      term = new Terminal({
        theme: termTheme,
        fontFamily: 'monospace', fontSize: 14, cursorBlink: true, scrollback: 1000,
        allowProposedApi: true,
        convertEol: true,
        disableStdin: false
      });
      if (term.options) {
        term.options.scrollOnUserInput = false;
      }
      fitAddon = new FitAddon();
      term.loadAddon(fitAddon);
      
      const unicode11Addon = new Unicode11Addon();
      term.loadAddon(unicode11Addon);
      if (term.unicode) {
        term.unicode.activeVersion = '11';
      }

      term.open(terminalRef.current);
      xtermRef.current = term;
      fitAddonRef.current = fitAddon;
      terminalMap.set(sessionId, term);
      fitAddonMap.set(sessionId, fitAddon);

      async function pollPty() {
        if (!pollActive || !isMounted || !pollStartedRef.current) return;
        try {
          const data = await invoke<string | null>("read_agent_pty", { sessionId });
          if (data && pollActive && isMounted && term) {
            await new Promise<void>(resolve => term!.write(data, resolve));
            if (pollActive && isMounted) requestAnimationFrame(pollPty);
          } else if (pollActive && isMounted) {
            setTimeout(pollPty, 50);
          }
        } catch (e) {
          if (!pollActive || !isMounted) return;
          console.warn("read_agent_pty error:", e);
          if (pollActive && isMounted) setTimeout(pollPty, 500);
        }
      }

      const checkSizingAndStart = () => {
        if (!isMounted || !fitAddon || !term || pollStartedRef.current) return;
        const el = terminalRef.current;
        if (!el || el.offsetParent === null || el.clientWidth < 10) return;
        
        try {
          fitAddon.fit();
          if (term.cols > 10 && term.rows > 3) {
            pollStartedRef.current = true;
            // Removed term.reset() to avoid clearing history on remount
            invoke("resize_agent_terminal", { sessionId, cols: term.cols, rows: term.rows })
              .then(() => { if (isMounted) requestAnimationFrame(pollPty); })
              .catch(() => { if (isMounted) requestAnimationFrame(pollPty); });
            term.focus();
          }
        } catch { /* ignore */ }
      };

      requestAnimationFrame(checkSizingAndStart);
      setTimeout(checkSizingAndStart, 50);
      setTimeout(checkSizingAndStart, 200);

      term.onData((data) => {
        if (data === '\x1b[I' || data === '\x1b[O') return;
        term!.scrollToBottom();
        emit('terminal-input', { sessionId, input: data });
      });

      term.onTitleChange((title) => {
        if (onTitleChangeRef.current) onTitleChangeRef.current(title);
      });

      let resizeTimeout: ReturnType<typeof setTimeout> | null = null;
      
      resizeObserver = new ResizeObserver(() => {
        if (!isMounted) return;
        if (!pollStartedRef.current) {
          checkSizingAndStart();
        }
        if (resizeTimeout) clearTimeout(resizeTimeout);
        resizeTimeout = setTimeout(() => {
          if (!isMounted) return;
          requestAnimationFrame(() => performFit());
        }, 16);
      });
      resizeObserver.observe(terminalRef.current);

      let ptyResizeTimeout: ReturnType<typeof setTimeout> | null = null;
      let lastCols = term.cols;
      let lastRows = term.rows;

      term.onResize((size) => {
        if (size.cols === lastCols && size.rows === lastRows) return;
        if (size.cols < 10 || size.rows < 2) return;
        lastCols = size.cols;
        lastRows = size.rows;
        if (ptyResizeTimeout) clearTimeout(ptyResizeTimeout);
        ptyResizeTimeout = setTimeout(() => {
          invoke("resize_agent_terminal", { sessionId, cols: size.cols, rows: size.rows }).catch(() => {});
        }, 50);
      });

      return () => {
        isMounted = false;
        pollActive = false;
        pollStartedRef.current = false;
        if (resizeTimeout) clearTimeout(resizeTimeout);
        if (ptyResizeTimeout) clearTimeout(ptyResizeTimeout);
        resizeObserver?.disconnect();
        terminalMap.delete(sessionId);
        fitAddonMap.delete(sessionId);
        term?.dispose();
        xtermRef.current = null;
        fitAddonRef.current = null;
      };
    } catch (e: any) {
      console.error("AgentTerminal Init Error:", e);
      setInitError(String(e));
    }
  }, [sessionId, termTheme, performFit]);

  useEffect(() => {
    const term = xtermRef.current;
    if (term) {
      term.options.theme = termTheme;
    }
  }, [termTheme]);

  useEffect(() => {
    let isMounted = true;
    const timers = [
      setTimeout(() => isMounted && performFit(true), 50),
      setTimeout(() => isMounted && performFit(true), 150),
      setTimeout(() => isMounted && performFit(true), 400),
    ];
    return () => {
      isMounted = false;
      timers.forEach(clearTimeout);
    };
  }, [sessionId, isMaximized, performFit]);

  return (
    <div className="relative w-full h-full overflow-hidden">
      {initError && (
        <div className="absolute inset-0 z-50 bg-red-900 text-primary p-4 overflow-auto rounded m-2">
          <h3 className="font-bold mb-2">Terminal Initialization Fatal Error:</h3>
          <pre className="text-xs whitespace-pre-wrap">{initError}</pre>
        </div>
      )}
      <div 
        ref={terminalRef} 
        onClick={() => xtermRef.current?.focus()} 
        className="w-full h-full overflow-hidden" 
        style={{ willChange: 'transform' }} 
      />
    </div>
  );
});
