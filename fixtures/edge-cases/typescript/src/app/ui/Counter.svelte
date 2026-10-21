<script lang="ts">
  import UserCard from './UserCard.vue';

  let count = 0;

  function increment(): void {
    count += 1;
  }
</script>

<button on:click={increment}>{count}</button>
<UserCard />
