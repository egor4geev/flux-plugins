<!-- A component with everything the queries handle: check with `tree-sitter highlight`. -->
<template>
  <div :class="{ active: isActive }" style="color: red" @click.prevent="toggle($event)">
    <h1 v-if="items.length > 0">{{ title.toUpperCase() }} &amp; more</h1>
    <ul>
      <li v-for="(item, index) in items" :key="item.id" v-on:[eventName]="select(index)">
        {{ index }}: {{ item.name }}
      </li>
    </ul>
    <slot #footer="{ total }">Total: {{ total }}</slot>
    <MyButton v-model.trim="query" disabled />
  </div>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'

const props = defineProps<{ title: string }>()
const items = ref<{ id: number; name: string }[]>([])
const isActive = computed(() => items.value.length > 0)
function toggle(event: MouseEvent): void {}
</script>

<script>
export default { name: 'Sample' }
</script>

<style scoped>
.active { color: #42b883; }
</style>

<style lang="scss">
$accent: #35495e;
.list { li { color: $accent; } }
</style>
