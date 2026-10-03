// 基础控件（规格 06 §4）。图标统一 Lucide，线宽 1.5px（Lucide 默认 2px 太粗不像苹果）。

import * as RadixSelect from '@radix-ui/react-select';
import * as RadixSlider from '@radix-ui/react-slider';
import * as RadixSwitch from '@radix-ui/react-switch';
import clsx from 'clsx';
import { Check, ChevronsUpDown, Search, X, type LucideIcon } from 'lucide-react';
import { forwardRef, type ButtonHTMLAttributes, type InputHTMLAttributes, type ReactNode } from 'react';

export function Icon({ icon: C, size = 16, className }: { icon: LucideIcon; size?: number; className?: string }) {
  return <C size={size} strokeWidth={1.5} absoluteStrokeWidth className={className} aria-hidden />;
}

type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'outline' | 'danger';

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: 'sm' | 'md' | 'lg';
  loading?: boolean;
  icon?: LucideIcon;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant = 'secondary', size = 'md', loading, icon, className, children, disabled, ...rest },
  ref,
) {
  return (
    <button
      ref={ref}
      type="button"
      className={clsx('cn-btn', `cn-btn--${variant}`, size !== 'md' && `cn-btn--${size}`, className)}
      disabled={disabled || loading}
      {...rest}
    >
      <span className={clsx('cn-btn__inner', loading && 'cn-btn__label--hidden')} style={{ display: 'contents' }}>
        {icon && <Icon icon={icon} size={size === 'lg' ? 16 : 14} />}
        {children}
      </span>
      {loading && (
        <span className="cn-btn__spinner">
          <Spinner size={14} />
        </span>
      )}
    </button>
  );
});

export interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  icon: LucideIcon;
  label: string;
  size?: 'sm' | 'md' | 'lg';
  active?: boolean;
}

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
  { icon, label, size = 'md', active, className, ...rest },
  ref,
) {
  const iconSize = size === 'sm' ? 14 : size === 'lg' ? 18 : 16;
  return (
    <button
      ref={ref}
      type="button"
      aria-label={label}
      data-active={active || undefined}
      className={clsx('cn-icon-btn', size !== 'md' && `cn-icon-btn--${size}`, className)}
      {...rest}
    >
      <Icon icon={icon} size={iconSize} />
    </button>
  );
});

export function Switch({
  checked,
  onChange,
  disabled,
  label,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  label?: string;
}) {
  return (
    <RadixSwitch.Root className="cn-switch" checked={checked} onCheckedChange={onChange} disabled={disabled} aria-label={label}>
      <RadixSwitch.Thumb className="cn-switch__thumb" />
    </RadixSwitch.Root>
  );
}

export const TextField = forwardRef<HTMLInputElement, InputHTMLAttributes<HTMLInputElement> & { leading?: ReactNode }>(
  function TextField({ leading, className, ...rest }, ref) {
    return (
      <label className={clsx('cn-field', className)}>
        {leading && <span className="cn-field__icon">{leading}</span>}
        <input ref={ref} spellCheck={false} autoComplete="off" {...rest} />
      </label>
    );
  },
);

export const SearchField = forwardRef<
  HTMLInputElement,
  Omit<InputHTMLAttributes<HTMLInputElement>, 'onChange' | 'value'> & { value: string; onChange: (v: string) => void }
>(function SearchField({ value, onChange, className, ...rest }, ref) {
  return (
    <label className={clsx('cn-field', className)}>
      <span className="cn-field__icon">
        <Icon icon={Search} size={14} />
      </span>
      <input
        ref={ref}
        type="text"
        spellCheck={false}
        autoComplete="off"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        {...rest}
      />
      {value && (
        <button type="button" className="cn-field__clear" aria-label="clear" onClick={() => onChange('')}>
          <X size={10} strokeWidth={2.5} />
        </button>
      )}
    </label>
  );
});

export function Segmented<T extends string>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: { value: T; label: ReactNode }[];
  onChange: (v: T) => void;
}) {
  return (
    <div className="cn-segmented" role="radiogroup">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={o.value === value}
          data-active={o.value === value}
          onClick={() => onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function Select<T extends string>({
  value,
  options,
  onChange,
  width,
  disabled,
}: {
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
  width?: number;
  disabled?: boolean;
}) {
  return (
    <RadixSelect.Root value={value} disabled={disabled} onValueChange={(v) => onChange(v as T)}>
      <RadixSelect.Trigger className="cn-select-trigger" style={width ? { minWidth: width } : undefined}>
        <RadixSelect.Value />
        <RadixSelect.Icon>
          <ChevronsUpDown size={12} strokeWidth={1.5} />
        </RadixSelect.Icon>
      </RadixSelect.Trigger>
      <RadixSelect.Portal>
        <RadixSelect.Content className="cn-menu" position="popper" sideOffset={4}>
          <RadixSelect.Viewport>
            {options.map((o) => (
              <RadixSelect.Item key={o.value} value={o.value} className="cn-menu__item">
                <span className="cn-menu__check">
                  <RadixSelect.ItemIndicator>
                    <Check size={12} strokeWidth={2} />
                  </RadixSelect.ItemIndicator>
                </span>
                <RadixSelect.ItemText>{o.label}</RadixSelect.ItemText>
              </RadixSelect.Item>
            ))}
          </RadixSelect.Viewport>
        </RadixSelect.Content>
      </RadixSelect.Portal>
    </RadixSelect.Root>
  );
}

export function Slider({
  value,
  min,
  max,
  step = 1,
  onChange,
  onCommit,
  width,
  label,
}: {
  value: number;
  min: number;
  max: number;
  step?: number;
  onChange: (v: number) => void;
  onCommit?: (v: number) => void;
  width?: number;
  label?: string;
}) {
  return (
    <RadixSlider.Root
      className="cn-slider"
      style={width ? { width } : undefined}
      value={[value]}
      min={min}
      max={max}
      step={step}
      onValueChange={(v) => onChange(v[0] ?? value)}
      onValueCommit={(v) => onCommit?.(v[0] ?? value)}
    >
      <RadixSlider.Track className="cn-slider__track">
        <RadixSlider.Range className="cn-slider__range" />
      </RadixSlider.Track>
      <RadixSlider.Thumb className="cn-slider__thumb" aria-label={label} />
    </RadixSlider.Root>
  );
}

/** 苹果那种 8 段辐条的加载指示器。 */
export function Spinner({ size = 16, className }: { size?: number; className?: string }) {
  return (
    <span className={clsx('cn-spinner', className)} style={{ width: size, height: size }} role="progressbar">
      {Array.from({ length: 8 }, (_, i) => (
        <span key={i} style={{ transform: `rotate(${i * 45}deg)`, animationDelay: `${(i - 8) * 0.1}s` }} />
      ))}
    </span>
  );
}

export function EmptyState({
  icon,
  title,
  description,
  action,
}: {
  icon: LucideIcon;
  title: string;
  description?: string;
  action?: ReactNode;
}) {
  return (
    <div className="cn-empty">
      <span className="cn-empty__icon">
        <Icon icon={icon} size={48} />
      </span>
      <div className="cn-empty__title">{title}</div>
      {description && <div className="cn-empty__desc">{description}</div>}
      {action}
    </div>
  );
}

export function Kbd({ keys }: { keys: string }) {
  const parts = keys.split('+').filter(Boolean);
  return (
    <span className="cn-kbd-group">
      {parts.map((k, i) => (
        <kbd key={i} className="cn-kbd">
          {k === 'Super' ? 'Win' : k}
        </kbd>
      ))}
    </span>
  );
}

export function Skeleton() {
  return (
    <div className="cn-skeleton" aria-busy>
      <span />
      <span />
      <span />
    </div>
  );
}
