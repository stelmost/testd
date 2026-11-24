<script setup lang="ts">
import { ref } from 'vue';

const props = defineProps<{
	labels: string[];
}>();

const active = ref(0);
</script>

<template>
	<div class="td-tabs">
		<div class="td-tabs__bar" role="tablist">
			<button
				v-for="(label, index) in props.labels"
				:key="label"
				type="button"
				role="tab"
				class="td-tabs__tab"
				:class="{ 'is-active': active === index }"
				:aria-selected="active === index"
				@click="active = index"
			>
				{{ label }}
			</button>
		</div>
		<div
			v-for="(label, index) in props.labels"
			v-show="active === index"
			:key="label"
			class="td-tabs__panel"
			role="tabpanel"
		>
			<slot :name="`tab-${index}`" />
		</div>
	</div>
</template>
