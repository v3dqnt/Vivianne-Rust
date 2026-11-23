#!/usr/bin/env node
// Turns the logos of a React app into files the Rust app draws.
//
//   node scripts/export-react-logos.mjs <source> [outDir]
//
// <source> is either
//   - a folder of .svg files: they are copied (named by their file name), or
//   - a module (.js/.mjs/.jsx; use `npx tsx` to run this for .ts/.tsx) whose exports are React components
//     that draw one logo each (`SlackLogo`, `Github`, ...). Each is rendered to static SVG with
//     react-dom/server, so run it from the React project (it needs `react` and `react-dom`).
//
// Files go to ~/.viv/brands/<id>.svg by default (or $VIV_HOME/brands). <id> is the component name lower-cased
// with a trailing Logo/Icon/Mark removed, and must be an app's id from Settings -> Connections (`slack`,
// `github`, ...). The app picks the files up the next time it draws them.

import { copyFileSync, existsSync, mkdirSync, readdirSync, statSync, writeFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { basename, extname, join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'

const [source, outArg] = process.argv.slice(2)
if (!source) {
  console.error('usage: export-react-logos.mjs <folder-of-svgs | react-module> [outDir]')
  process.exit(2)
}
const out = outArg ?? join(process.env.VIV_HOME ?? join(homedir(), '.viv'), 'brands')
mkdirSync(out, { recursive: true })

const idOf = (name) =>
  name
    .replace(/([a-z0-9])([A-Z])/g, '$1_$2')
    .toLowerCase()
    .replace(/[^a-z0-9_]+/g, '_')
    .replace(/_(logo|icon|mark|color)$/, '')
    .replace(/^_+|_+$/g, '')

let count = 0
const path = resolve(source)
if (statSync(path).isDirectory()) {
  for (const file of readdirSync(path)) {
    if (extname(file).toLowerCase() !== '.svg') continue
    copyFileSync(join(path, file), join(out, `${idOf(basename(file, '.svg'))}.svg`))
    count++
  }
} else {
  const React = (await import('react')).default
  const { renderToStaticMarkup } = await import('react-dom/server')
  const mod = await import(pathToFileURL(path).href)
  for (const [name, Component] of Object.entries(mod)) {
    if (typeof Component !== 'function' && typeof Component !== 'object') continue
    let svg
    try {
      svg = renderToStaticMarkup(React.createElement(Component, { size: 128, width: 128, height: 128 }))
    } catch {
      continue // not a component that renders on its own
    }
    if (!svg.startsWith('<svg')) continue
    if (!svg.includes('xmlns=')) svg = svg.replace('<svg', '<svg xmlns="http://www.w3.org/2000/svg"')
    writeFileSync(join(out, `${idOf(name)}.svg`), svg)
    count++
  }
}
console.log(`${count} logo(s) written to ${out}`)
if (!existsSync(out)) process.exit(1)
