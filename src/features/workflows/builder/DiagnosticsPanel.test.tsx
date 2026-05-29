import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { DiagnosticsPanel } from './DiagnosticsPanel';

describe('DiagnosticsPanel', () => {
  it('lists diagnostics and focuses a node on click', () => {
    const onFocus = vi.fn();
    render(<DiagnosticsPanel diagnostics={[{ severity: 'error', code: 'missing_required_field', message: 'node `plan` missing `prompt`', node: 'plan' }]} onFocusNode={onFocus} />);
    expect(screen.getByText(/missing_required_field/)).toBeInTheDocument();
    fireEvent.click(screen.getByText(/missing_required_field/));
    expect(onFocus).toHaveBeenCalledWith('plan');
  });
  it('shows a clean state when empty', () => {
    render(<DiagnosticsPanel diagnostics={[]} onFocusNode={() => {}} />);
    expect(screen.getByText(/no issues/i)).toBeInTheDocument();
  });
});
