<template>
  <section>
    <UserCard v-for="u in users" :key="u.id" />
    <user-card />
    <button @click="refresh">Reload</button>
    <p v-if="ready">{{ formatTotal(total) }}</p>
  </section>
</template>

<script setup lang="ts">
import UserCard from './UserCard.vue';

const users = [{ id: 1 }];
const ready = true;
const total = 3;

function refresh(): void {}

function formatTotal(n: number): string {
  return String(n);
}

function unused(): void {}
</script>

<style scoped>
.card { color: red; }
</style>
