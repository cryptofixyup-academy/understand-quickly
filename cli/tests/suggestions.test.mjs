import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { detectPrimaryLanguage, suggestFormats } from '../src/suggestions.mjs';

test('detectPrimaryLanguage: JavaScript/TypeScript', () => {
  const root = mkdtempSync(join(tmpdir(), 'uq-suggestions-'));
  writeFileSync(join(root, 'package.json'), '{}');
  const lang = detectPrimaryLanguage(root);
  assert.equal(lang, 'JavaScript/TypeScript');
});

test('detectPrimaryLanguage: Python', () => {
  const root = mkdtempSync(join(tmpdir(), 'uq-suggestions-'));
  writeFileSync(join(root, 'requirements.txt'), '');
  const lang = detectPrimaryLanguage(root);
  assert.equal(lang, 'Python');
});

test('detectPrimaryLanguage: Rust', () => {
  const root = mkdtempSync(join(tmpdir(), 'uq-suggestions-'));
  writeFileSync(join(root, 'Cargo.toml'), '');
  const lang = detectPrimaryLanguage(root);
  assert.equal(lang, 'Rust');
});

test('detectPrimaryLanguage: Go', () => {
  const root = mkdtempSync(join(tmpdir(), 'uq-suggestions-'));
  writeFileSync(join(root, 'go.mod'), '');
  const lang = detectPrimaryLanguage(root);
  assert.equal(lang, 'Go');
});

test('detectPrimaryLanguage: returns null for unknown', () => {
  const root = mkdtempSync(join(tmpdir(), 'uq-suggestions-'));
  const lang = detectPrimaryLanguage(root);
  assert.equal(lang, null);
});

test('suggestFormats: returns suggestions for known language', () => {
  const suggestions = suggestFormats('Python');
  assert.ok(Array.isArray(suggestions));
  assert.ok(suggestions.length > 0);
  assert.equal(suggestions[0].name, 'Understand-Anything');
});

test('suggestFormats: returns default for unknown language', () => {
  const suggestions = suggestFormats('Unknown');
  assert.ok(Array.isArray(suggestions));
  assert.equal(suggestions[0].name, 'Understand-Anything');
});

test('suggestFormats: returns default for null', () => {
  const suggestions = suggestFormats(null);
  assert.ok(Array.isArray(suggestions));
  assert.equal(suggestions[0].name, 'Understand-Anything');
});

test('suggestFormats: format has required fields', () => {
  const suggestions = suggestFormats('JavaScript/TypeScript');
  for (const s of suggestions) {
    assert.ok(s.name);
    assert.ok(s.format);
    assert.ok(s.description);
    assert.ok(s.url);
    assert.ok(s.command);
  }
});
