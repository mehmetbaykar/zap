import { defineCollection, z } from 'astro:content';
import { glob } from 'astro/loaders';

const docs = defineCollection({
  loader: glob({ pattern: '**/*.{md,mdx}', base: './src/content/docs' }),
  schema: z.object({
    title: z.string(),
    lede: z.string(),
    order: z.number().default(0),
    section: z.string().default('Get Started'),
  }),
});

export const collections = { docs };
