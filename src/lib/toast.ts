/** 提示停留多久：报错要读得完，长的多留一会儿（3–6 秒），其他 1.5 秒 */
export function toastStay(kind: 'success' | 'error' | 'info', message: string): number {
  if (kind !== 'error') return 1500;
  return Math.min(6000, Math.max(3000, 1500 + message.length * 80));
}
