import { describe, expect, it } from 'vitest';

import { normalizeAiBaseUrl } from './aiUrl';

describe('normalizeAiBaseUrl', () => {
  it('补上缺的版本号', () => {
    expect(normalizeAiBaseUrl('https://apijing.com', 'openai')).toBe('https://apijing.com/v1');
    expect(normalizeAiBaseUrl('https://apijing.com/', 'openai')).toBe('https://apijing.com/v1');
    expect(normalizeAiBaseUrl('https://openrouter.ai/api', 'openai')).toBe('https://openrouter.ai/api/v1');
  });

  it('已经填到版本号的不动', () => {
    expect(normalizeAiBaseUrl('https://api.openai.com/v1', 'openai')).toBe('https://api.openai.com/v1');
    expect(normalizeAiBaseUrl('https://open.bigmodel.cn/api/paas/v4', 'openai')).toBe('https://open.bigmodel.cn/api/paas/v4');
    expect(normalizeAiBaseUrl('https://dashscope.aliyuncs.com/compatible-mode/v1/', 'openai')).toBe(
      'https://dashscope.aliyuncs.com/compatible-mode/v1',
    );
    expect(normalizeAiBaseUrl('https://generativelanguage.googleapis.com/v1beta/openai', 'openai')).toBe(
      'https://generativelanguage.googleapis.com/v1beta/openai',
    );
  });

  it('整个接口地址贴进来时去掉多的那截', () => {
    expect(normalizeAiBaseUrl('https://api.deepseek.com/v1/chat/completions', 'openai')).toBe('https://api.deepseek.com/v1');
    expect(normalizeAiBaseUrl('https://api.deepseek.com/chat/completions', 'openai')).toBe('https://api.deepseek.com/v1');
  });

  it('补协议头', () => {
    expect(normalizeAiBaseUrl('api.moonshot.cn', 'openai')).toBe('https://api.moonshot.cn/v1');
    expect(normalizeAiBaseUrl('localhost:11434', 'openai')).toBe('http://localhost:11434/v1');
  });

  it('Anthropic 填到域名为止', () => {
    expect(normalizeAiBaseUrl('https://api.anthropic.com/v1/messages', 'anthropic')).toBe('https://api.anthropic.com');
    expect(normalizeAiBaseUrl('https://api.anthropic.com/', 'anthropic')).toBe('https://api.anthropic.com');
    expect(normalizeAiBaseUrl('https://proxy.example.com/anthropic/v1', 'anthropic')).toBe('https://proxy.example.com/anthropic');
  });

  it('空的保持空', () => {
    expect(normalizeAiBaseUrl('  ', 'openai')).toBe('');
  });
});
