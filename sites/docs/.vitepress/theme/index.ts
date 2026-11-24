import DefaultTheme from 'vitepress/theme';
import type { Theme } from 'vitepress';
import '@fontsource-variable/archivo';
import '@fontsource-variable/jetbrains-mono';
import DocTabs from './DocTabs.vue';
import './custom.css';

export default {
	extends: DefaultTheme,
	enhanceApp({ app }) {
		app.component('DocTabs', DocTabs);
	},
} satisfies Theme;
