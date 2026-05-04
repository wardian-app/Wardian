import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ScheduleEditor } from './ScheduleEditor';

const lastChange = (onChange: ReturnType<typeof vi.fn>) => onChange.mock.calls[onChange.mock.calls.length - 1][0];

describe('ScheduleEditor', () => {
  beforeEach(() => {
    vi.useRealTimers();
  });

  it('converts interval values between hours and minutes', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const { rerender } = render(<ScheduleEditor value={{ schedule_type: 'interval', interval_minutes: 120 }} onChange={onChange} />);

    fireEvent.change(screen.getByRole('spinbutton'), { target: { value: '3' } });
    expect(lastChange(onChange)).toMatchObject({ schedule_type: 'interval', interval_minutes: 180 });

    rerender(<ScheduleEditor value={{ schedule_type: 'interval', interval_minutes: 180 }} onChange={onChange} />);
    await user.selectOptions(screen.getAllByRole('combobox')[1], 'minutes');
    expect(lastChange(onChange)).toMatchObject({ schedule_type: 'interval', interval_minutes: 180 });
  });

  it('updates weekly days, repeat count, time, and end conditions', async () => {
    const user = userEvent.setup();
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.setSystemTime(new Date('2026-05-04T12:00:00Z'));
    const onChange = vi.fn();
    render(
      <ScheduleEditor
        value={{
          schedule_type: 'weekly',
          days_of_week: ['Mon'],
          repeat_every: 1,
          time_of_day: '09:00',
          active: true,
        }}
        onChange={onChange}
      />,
    );

    await user.click(screen.getByRole('button', { name: 'Wed' }));
    expect(lastChange(onChange)).toMatchObject({ days_of_week: ['Mon', 'Wed'] });

    fireEvent.change(screen.getByRole('spinbutton'), { target: { value: '2' } });
    expect(lastChange(onChange)).toMatchObject({ repeat_every: 2 });

    fireEvent.change(screen.getByDisplayValue('09:00'), { target: { value: '14:30' } });
    expect(lastChange(onChange)).toMatchObject({ time_of_day: '14:30' });

    await user.click(screen.getByRole('radio', { name: 'On date' }));
    expect(lastChange(onChange)).toMatchObject({ end_condition: 'on_date', end_date: '2026-05-04' });
  });

  it('sorts monthly day toggles', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<ScheduleEditor value={{ schedule_type: 'monthly', days_of_month: [15] }} onChange={onChange} />);

    await user.click(screen.getByRole('button', { name: '1' }));

    expect(lastChange(onChange)).toMatchObject({ days_of_month: [1, 15] });
  });

  it('adds, edits, and removes specific dates without rendering end conditions', async () => {
    const user = userEvent.setup();
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.setSystemTime(new Date('2026-05-04T12:00:00Z'));
    const onChange = vi.fn();
    const { rerender } = render(
      <ScheduleEditor
        value={{ schedule_type: 'specific_dates', specific_dates: [], time_of_day: '09:00' }}
        onChange={onChange}
      />,
    );

    await user.click(screen.getByRole('button', { name: /Add Date/i }));
    expect(lastChange(onChange)).toMatchObject({ specific_dates: ['2026-05-04'] });

    rerender(
      <ScheduleEditor
        value={{ schedule_type: 'specific_dates', specific_dates: ['2026-05-04'], time_of_day: '09:00' }}
        onChange={onChange}
      />,
    );
    fireEvent.change(screen.getByDisplayValue('2026-05-04'), { target: { value: '2026-05-05' } });
    expect(lastChange(onChange)).toMatchObject({ specific_dates: ['2026-05-05'] });

    rerender(
      <ScheduleEditor
        value={{ schedule_type: 'specific_dates', specific_dates: ['2026-05-05'], time_of_day: '09:00' }}
        onChange={onChange}
      />,
    );
    await user.click(within(screen.getByDisplayValue('2026-05-05').closest('div')!).getByRole('button'));
    expect(lastChange(onChange)).toMatchObject({ specific_dates: [] });
    expect(screen.queryByText('Ends')).not.toBeInTheDocument();
  });

  it('updates one-time run datetime and hides end conditions', () => {
    const onChange = vi.fn();
    render(<ScheduleEditor value={{ schedule_type: 'one_time', run_at: '' }} onChange={onChange} />);

    fireEvent.change(screen.getByLabelText('Date & Time'), {
      target: { value: '2026-05-04T14:30' },
    });

    expect(lastChange(onChange)).toMatchObject({ run_at: '2026-05-04T14:30' });
    expect(screen.queryByText('Ends')).not.toBeInTheDocument();
  });
});
