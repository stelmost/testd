import type { MarkdownOptions } from 'vitepress';

/*
	Code highlighting in the product page's colours: commands and headings in sand, flags and keys in
	teal, strings and values in violet. The hex values are `sites/tokens.css`'s; Shiki cannot read
	CSS variables, so they are repeated here.
*/
const TEXT = '#ecf2f9';
const COMMENT = '#878c93';
const SAND = '#d2b98f';
const TEAL = '#5bb5aa';
const VIOLET = '#9c92d8';
const LINK = '#8fddf0';
const AMBER = '#ffb27f';

const night: NonNullable<MarkdownOptions['theme']> = {
	name: 'testd-night',
	type: 'dark',
	colors: {
		'editor.background': '#070b11',
		'editor.foreground': TEXT,
	},
	tokenColors: [
		{ scope: ['comment', 'punctuation.definition.comment'], settings: { foreground: COMMENT } },
		{ scope: ['entity.name', 'support.function'], settings: { foreground: SAND } },
		{
			scope: ['constant.other.option', 'variable.other.assignment', 'variable.parameter'],
			settings: { foreground: TEAL },
		},
		{ scope: ['string'], settings: { foreground: VIOLET } },
		{ scope: ['keyword', 'storage'], settings: { foreground: LINK } },
		{ scope: ['constant.numeric', 'constant.language'], settings: { foreground: AMBER } },
	],
};

export default night;
