import { render, screen } from '@testing-library/react';
import { describe, it, expect, vi } from 'vitest';
import { ClassManagerPanel } from './ClassManagerPanel';
import { AgentClassDefinition } from '../../types';

// Mock the Tauri invoke command
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

// Mock ManageSkills to avoid rendering complex nested state logic
vi.mock('../library/ManageSkills', () => ({
  ManageSkills: () => <div data-testid="manage-skills-mock">Manage Skills</div>
}));

describe('ClassManagerPanel', () => {
  const mockClasses: AgentClassDefinition[] = [
    {
      name: 'Coder',
      description: 'Default coder class',
      is_default: true,
    },
    {
      name: 'CustomAgent',
      description: 'A custom agent class',
      is_default: false,
    }
  ];

  it('renders both default and custom classes in a unified list', () => {
    render(<ClassManagerPanel agentClasses={mockClasses} onClassesUpdated={vi.fn()} />);
    
    expect(screen.getByText('Available Classes')).toBeInTheDocument();
    expect(screen.getByText('Coder')).toBeInTheDocument();
    expect(screen.getByText('CustomAgent')).toBeInTheDocument();
  });

  it('renders the Default badge only for default classes', () => {
    render(<ClassManagerPanel agentClasses={mockClasses} onClassesUpdated={vi.fn()} />);
    
    const defaultBadges = screen.getAllByText('Default');
    expect(defaultBadges).toHaveLength(1); // Only one default class
  });

  it('renders the delete button only for custom classes', () => {
    const { container } = render(<ClassManagerPanel agentClasses={mockClasses} onClassesUpdated={vi.fn()} />);
    
    // There should be exactly one delete button (for the CustomAgent)
    const deleteButtons = container.querySelectorAll('button[title="Delete class"]');
    expect(deleteButtons).toHaveLength(1);
  });

  it('renders the ManageSkills component for all classes', () => {
    render(<ClassManagerPanel agentClasses={mockClasses} onClassesUpdated={vi.fn()} />);
    
    const manageSkillsMocks = screen.getAllByTestId('manage-skills-mock');
    expect(manageSkillsMocks).toHaveLength(2); // One for each class
  });
});