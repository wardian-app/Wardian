import { render, screen, fireEvent } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { SectionRail } from './SectionRail';
import { LibraryEntry, LibraryIndex } from '../../types';

function entry(overrides: Partial<LibraryEntry> & Pick<LibraryEntry, 'name' | 'path' | 'entry_ref'>): LibraryEntry {
  return {
    kind: 'skill',
    description: '',
    tags: [],
    is_starred: false,
    deployment_count: 0,
    error: null,
    ...overrides,
  };
}

const sections: LibraryIndex['sections'] = {
  skills: {
    stubbed: false,
    tree: {
      path: '',
      name: 'Root',
      children: [
        entry({ name: 'planner', path: 'dev/planner', entry_ref: 'skills/dev/planner' }),
        {
          path: 'dev',
          name: 'dev',
          children: [entry({ name: 'reviewer', path: 'dev/reviewer', entry_ref: 'skills/dev/reviewer' })],
        },
      ],
    },
  },
  prompts: {
    stubbed: false,
    tree: { path: '', name: 'Root', children: [] },
  },
  workflows: {
    stubbed: false,
    tree: { path: '', name: 'Root', children: [] },
  },
  classes: {
    stubbed: false,
    tree: { path: '', name: 'Root', children: [] },
  },
  mcps: {
    stubbed: true,
    tree: { path: '', name: 'Root', children: [] },
  },
};

describe('SectionRail', () => {
  it('renders all five sections', () => {
    render(<SectionRail activeSection="skills" sections={null} onSelect={vi.fn()} />);
    for (const id of ['skills', 'prompts', 'classes', 'workflows', 'mcps']) {
      expect(screen.getByTestId(`library-section-${id}`)).toBeInTheDocument();
    }
  });

  it('shows a recursive entry count per section, hiding the badge when zero', () => {
    render(<SectionRail activeSection="skills" sections={sections} onSelect={vi.fn()} />);

    const skillsButton = screen.getByTestId('library-section-skills');
    expect(skillsButton).toHaveTextContent('2');

    const promptsButton = screen.getByTestId('library-section-prompts');
    expect(promptsButton).not.toHaveTextContent('0');
  });

  it('renders count as zero (no badge) when sections is null', () => {
    render(<SectionRail activeSection="skills" sections={null} onSelect={vi.fn()} />);
    const skillsButton = screen.getByTestId('library-section-skills');
    expect(skillsButton.querySelector('.text-muted-neutral')).not.toBeInTheDocument();
  });

  it('fires onSelect with the section id when a button is clicked', () => {
    const onSelect = vi.fn();
    render(<SectionRail activeSection="skills" sections={sections} onSelect={onSelect} />);

    fireEvent.click(screen.getByTestId('library-section-prompts'));
    expect(onSelect).toHaveBeenCalledWith('prompts');
  });

  it('applies the active styling only to the active section', () => {
    render(<SectionRail activeSection="prompts" sections={sections} onSelect={vi.fn()} />);

    expect(screen.getByTestId('library-section-prompts').className).toContain(
      'border-[var(--color-wardian-accent)]',
    );
    expect(screen.getByTestId('library-section-skills').className).toContain('border-transparent');
  });

  it('marks only the active section with aria-current', () => {
    render(<SectionRail activeSection="prompts" sections={sections} onSelect={vi.fn()} />);

    expect(screen.getByTestId('library-section-prompts')).toHaveAttribute('aria-current', 'true');
    expect(screen.getByTestId('library-section-skills')).not.toHaveAttribute('aria-current');
  });

  it('renders every section label unclipped, including the longest label ("Workflows")', () => {
    render(<SectionRail activeSection="skills" sections={sections} onSelect={vi.fn()} />);

    for (const [id, label] of [
      ['skills', 'Skills'],
      ['prompts', 'Prompts'],
      ['classes', 'Classes'],
      ['workflows', 'Workflows'],
      ['mcps', 'MCPs'],
    ] as const) {
      expect(screen.getByTestId(`library-section-${id}`)).toHaveTextContent(label);
    }

    // Regression guard: the rail must be wide enough (and the label's
    // tracking loose enough) that "Workflows" doesn't clip to "Workflow".
    const rail = screen.getByTestId('library-section-rail');
    expect(rail.className).not.toContain('w-14');
  });
});
