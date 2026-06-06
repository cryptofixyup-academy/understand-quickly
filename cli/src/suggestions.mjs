// Language detection and format suggestions.
// Helps users choose the right tool if they don't have a graph yet.

import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

// Detect primary language by analyzing key files in the repo.
export function detectPrimaryLanguage(repoRoot) {
  // Check for language indicators, in order of prevalence
  const indicators = [
    { files: ['package.json'], lang: 'JavaScript/TypeScript' },
    { files: ['yarn.lock', 'pnpm-lock.yaml'], lang: 'JavaScript/TypeScript' },
    { files: ['Cargo.toml'], lang: 'Rust' },
    { files: ['go.mod'], lang: 'Go' },
    { files: ['requirements.txt', 'setup.py', 'pyproject.toml'], lang: 'Python' },
    { files: ['Gemfile'], lang: 'Ruby' },
    { files: ['pom.xml', 'build.gradle'], lang: 'Java' },
    { files: ['composer.json'], lang: 'PHP' },
    { files: ['.csproj', '.sln'], lang: 'C#' },
    { files: ['CMakeLists.txt'], lang: 'C/C++' },
    { files: ['Makefile'], lang: 'C/C++' },
    { files: ['tsconfig.json'], lang: 'TypeScript' },
    { files: ['.eslintrc', '.prettierrc'], lang: 'JavaScript' }
  ];

  for (const { files, lang } of indicators) {
    for (const file of files) {
      if (existsSync(join(repoRoot, file))) {
        return lang;
      }
    }
  }

  return null;
}

// Get suggested tools and formats for a given language.
export function suggestFormats(language) {
  if (!language) {
    return [
      {
        name: 'Understand-Anything',
        format: 'understand-anything@1',
        description: 'Works with any language. Auto-detects code structure.',
        url: 'https://github.com/Lum1104/Understand-Anything',
        command: 'npx understand-anything-docker'
      }
    ];
  }

  const suggestions = {
    'JavaScript/TypeScript': [
      {
        name: 'Understand-Anything',
        format: 'understand-anything@1',
        description: 'Full code structure + concepts. Best for general use.',
        url: 'https://github.com/Lum1104/Understand-Anything',
        command: 'npx understand-anything-docker'
      },
      {
        name: 'GitNexus',
        format: 'gitnexus@1',
        description: 'Includes git history. Best for tracking changes over time.',
        url: 'https://github.com/abhigyanpatwari/GitNexus',
        command: 'npx gitnexus'
      }
    ],
    'Python': [
      {
        name: 'Understand-Anything',
        format: 'understand-anything@1',
        description: 'Full code structure + concepts. Best for general use.',
        url: 'https://github.com/Lum1104/Understand-Anything',
        command: 'npx understand-anything-docker'
      },
      {
        name: 'Repomix',
        format: 'bundle@1',
        description: 'Packed text dump. Good for whole-repo context in LLMs.',
        url: 'https://github.com/yamadashy/repomix',
        command: 'npx repomix'
      }
    ],
    'Rust': [
      {
        name: 'Understand-Anything',
        format: 'understand-anything@1',
        description: 'Full code structure + concepts. Best for general use.',
        url: 'https://github.com/Lum1104/Understand-Anything',
        command: 'npx understand-anything-docker'
      }
    ],
    'Go': [
      {
        name: 'Understand-Anything',
        format: 'understand-anything@1',
        description: 'Full code structure + concepts. Best for general use.',
        url: 'https://github.com/Lum1104/Understand-Anything',
        command: 'npx understand-anything-docker'
      },
      {
        name: 'GitNexus',
        format: 'gitnexus@1',
        description: 'Includes git history. Best for tracking changes over time.',
        url: 'https://github.com/abhigyanpatwari/GitNexus',
        command: 'npx gitnexus'
      }
    ],
    'Java': [
      {
        name: 'Understand-Anything',
        format: 'understand-anything@1',
        description: 'Full code structure + concepts. Best for general use.',
        url: 'https://github.com/Lum1104/Understand-Anything',
        command: 'npx understand-anything-docker'
      }
    ],
    'C/C++': [
      {
        name: 'Understand-Anything',
        format: 'understand-anything@1',
        description: 'Full code structure + concepts. Best for general use.',
        url: 'https://github.com/Lum1104/Understand-Anything',
        command: 'npx understand-anything-docker'
      }
    ]
  };

  return suggestions[language] || suggestions['JavaScript/TypeScript'];
}

// Format suggestion string for display.
export function formatSuggestion(suggestion) {
  return `${suggestion.name} (${suggestion.format}): ${suggestion.description}
  Install: ${suggestion.command}
  Learn more: ${suggestion.url}`;
}

// Print suggestions to stderr.
export function printSuggestions(language) {
  const suggestions = suggestFormats(language);
  const langStr = language ? ` for ${language}` : '';

  console.error(`\n📊 No graph found yet. Here are tools to generate one${langStr}:\n`);
  suggestions.forEach((s, i) => {
    console.error(`${i + 1}. ${formatSuggestion(s)}\n`);
  });
}
