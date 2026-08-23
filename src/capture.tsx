import { createRoot } from 'react-dom/client';

import { CaptureOverlay } from '@/features/capture/CaptureOverlay';
import '@/styles/capture.css';

const container = document.getElementById('root');
if (!container) throw new Error('找不到挂载点 #root');

// 刻意不套 StrictMode：它会把 effect 跑两遍，这里的 effect 要抓一整屏底图
// （4K 屏 33MB），跑两遍会白白多花几十毫秒，而从热键到上屏的预算只有 150ms。
createRoot(container).render(<CaptureOverlay />);
