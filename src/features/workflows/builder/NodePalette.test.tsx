import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { NodePalette } from './NodePalette';

describe('NodePalette', () => {
  it('renders an entry per node type grouped by category', () => {
    render(<NodePalette onAdd={() => {}} />);
    expect(screen.getByText('Task')).toBeInTheDocument();
    expect(screen.getByText('Approval')).toBeInTheDocument();
    expect(screen.getByText(/Control/i)).toBeInTheDocument(); // a category header
  });
});
