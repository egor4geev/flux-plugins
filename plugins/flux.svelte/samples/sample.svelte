<!-- A component with everything the queries handle: check with `tree-sitter highlight`. -->
<script lang="ts">
  let { items = [] }: { items: { id: number; name: string }[] } = $props();
  let count = $state(0);
</script>

<script module>
  export const prerender = true;
</script>

<h1 class="title {count > 0 ? 'active' : ''}" style="color: red">Hello &amp; {items.length}</h1>

{#if count > 10}
  <p>Big</p>
{:else if count > 5}
  <p>Medium</p>
{:else}
  <p>Small</p>
{/if}

{#each items as item, i (item.id)}
  <button onclick={() => count++} {disabled}>{i}: {item.name}</button>
{/each}

{#await fetch('/api') then response}
  {@html response.statusText}
{/await}

{#snippet row(item)}
  {@const label = item.name.toUpperCase()}
  <td>{label}</td>
{/snippet}
{@render row(items[0])}

<style lang="scss">
  $accent: #ff3e00;
  h1 { color: $accent; }
</style>
