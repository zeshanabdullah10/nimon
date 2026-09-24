import { clsx, type ClassValue } from 'clsx'

/* Plain clsx: class lists are authored without conflicts, so no
 * tailwind-merge runtime is shipped in the single-file bundle. */
export function cn(...inputs: ClassValue[]) {
  return clsx(inputs)
}
