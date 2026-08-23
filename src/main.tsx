import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

import { DiagnosticsPage } from '@/app/dev/DiagnosticsPage';
import '@/styles/main.css';

const container = document.getElementById('root');
if (!container) throw new Error('找不到挂载点 #root');

createRoot(container).render(
  <StrictMode>
    <DiagnosticsPage />
  </StrictMode>,
);
