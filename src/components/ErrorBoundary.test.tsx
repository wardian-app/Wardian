import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ErrorBoundary } from './ErrorBoundary';

const ProblemChild = () => {
  throw new Error('render failed');
};

describe('ErrorBoundary', () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('renders children when no error is thrown', () => {
    render(
      <ErrorBoundary>
        <div>Healthy UI</div>
      </ErrorBoundary>,
    );

    expect(screen.getByText('Healthy UI')).toBeInTheDocument();
  });

  it('shows the fatal error fallback and reloads the app', async () => {
    const user = userEvent.setup();
    vi.spyOn(console, 'error').mockImplementation(() => {});
    const reload = vi.fn();
    vi.spyOn(window, 'location', 'get').mockReturnValue({
      ...window.location,
      reload,
    } as Location);

    render(
      <ErrorBoundary>
        <ProblemChild />
      </ErrorBoundary>,
    );

    expect(screen.getByText('Fatal UI Rendering Error')).toBeInTheDocument();
    expect(screen.getByText('Error: render failed')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Reload Wardian' }));
    expect(reload).toHaveBeenCalled();
  });
});
