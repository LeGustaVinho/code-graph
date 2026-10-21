<template>
  <button @click="save">{{ title }}</button>
</template>

<script>
export default {
  name: 'Options',
  data() {
    return { title: 'x' };
  },
  methods: {
    save() {
      this.persist();
    },
    persist() {},
  },
  mounted() {
    this.save();
  },
};
</script>
