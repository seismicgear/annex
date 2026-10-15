import { defineConfig } from '@playwright/test';
import { fileURLToPath } from 'node:url';
const outputDir=fileURLToPath(new URL('../../e2e-results/boundaries/',import.meta.url));
export default defineConfig({testDir:'.',testMatch:'boundaries.live.ts',fullyParallel:false,workers:1,retries:0,forbidOnly:true,
  timeout:900_000,expect:{timeout:25_000},outputDir,
  reporter:[['list',{printSteps:true}],['json',{outputFile:`${outputDir}/results.json`}]],
  use:{baseURL:process.env.ANNEX_LIVE_URL??'http://127.0.0.1:3000',actionTimeout:30_000,trace:'off',screenshot:'off',video:'off'},
  projects:[{name:'live-boundaries',use:{browserName:'chromium'}}]});
